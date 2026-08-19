#!/usr/bin/env bash
#
# Two-session isolation e2e gate for agent-terminal through the pi and omp
# coding agents.
#
# Launches TWO independent agent sessions against TWO local fixture servers,
# each driving the same agent-terminal lifecycle (with the hold step enabled)
# using the SAME job name `server`. While both jobs are held mid-lifecycle,
# the script cross-checks each session's scope from the host side. The gate
# proves:
#   - sessions with different scopes never share state: each session's
#     agent-terminal CLI sees only its own job, and each scope's Zellij
#     server is independent (state roots and socket dirs are derived from the
#     scope digest);
#   - the fixture's exact job-name override (AGENT_TERMINAL_JOB_NAME=server)
#     is honored end to end.
#
# No model is involved: the fixture provider extension replaces the model,
# exactly like scripts/e2e-pi-local.sh (a real pi/omp binary, real extension
# hooks, real Bash execution, real agent-terminal).
#
# Usage:
#   AGENT_TERMINAL_AGENT=pi bash scripts/e2e-two-session.sh
#   AGENT_TERMINAL_AGENT=omp bash scripts/e2e-two-session.sh
#
# Environment overrides:
#   AGENT_TERMINAL_AGENT             - agent under test: pi (default) or omp
#   AGENT_TERMINAL_BIN               - agent-terminal binary to test (default:
#                                      target/x86_64-unknown-linux-musl/release/agent-terminal)
#   AGENT_TERMINAL_FIXTURE_HOLD_SECS - hold sleep duration for the fixtures
#                                      (default: 45; must be >= 30)
#   AGENT_TERMINAL_CLEANUP           - delete temp dirs on exit (default: 0)
#   AGENT_TERMINAL_SKIP_PREFLIGHT    - skip the agent package build (default: 0)
#   AGENT_TERMINAL_PROMPT_E2E_TIMEOUT - per-agent run timeout (default: 600)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly RUN_ID="$$"
readonly SANDBOX="/tmp/agent-terminal-2session-$RUN_ID"
readonly ARTIFACT_DIR="$SANDBOX/artifacts"
readonly WORKDIR="$SANDBOX/workdir"
readonly CONFIG_DIR="$SANDBOX/config"
readonly DATA_DIR="$SANDBOX/data"
readonly CACHE_DIR="$SANDBOX/cache"
readonly STATE_DIR="$SANDBOX/state"
# ONE shared Zellij socket directory for both sessions: the gate must exercise
# two scopes colliding in a single socket namespace. The agent-terminal CLI
# ignores ZELLIJ_SOCKET_DIR (src/paths.rs derives per-scope socket dirs from
# the scope digest under the OS temp dir), so the variable itself is inert;
# it is still passed everywhere to document the shared-namespace intent and to
# keep any future env-respecting build honest.
readonly SOCKET_DIR="$SANDBOX/socket"
readonly JOB_NAME="server"
readonly FIXTURE_SCRIPT="$REPO_ROOT/pi/scripts/e2e-fixture.ts"

CLEANUP="${AGENT_TERMINAL_CLEANUP:-0}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"
AGENT_TERMINAL_AGENT="${AGENT_TERMINAL_AGENT:-pi}"
FIXTURE_HOLD_SECS="${AGENT_TERMINAL_FIXTURE_HOLD_SECS:-45}"
PROMPT_E2E_TIMEOUT="${AGENT_TERMINAL_PROMPT_E2E_TIMEOUT:-600}"

AT_BIN=""
AGENT_BIN=""
FIXTURE_A_PID=""
FIXTURE_B_PID=""
declare -A AGENT_PIDS
EVIDENCE_DIR=""

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

PHASE="init"
phase() { PHASE="$1"; }
fail_phase() {
  printf 'error: phase "%s" failed\n' "$PHASE" >&2
  exit 1
}
trap fail_phase ERR

cleanup() {
  local exit_status=$?
  local sessions=""
  local session=""

  trap - EXIT INT TERM HUP ERR
  set +e

  for pid in "${AGENT_PIDS[@]}" $FIXTURE_A_PID $FIXTURE_B_PID; do
    if [[ -n $pid ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done

  # Preserve evidence BEFORE the run-specific socket cleanup below.
  if [[ $CLEANUP != 1 && -d $ARTIFACT_DIR ]]; then
    EVIDENCE_DIR="/tmp/agent-terminal-2session-$RUN_ID-evidence"
    rm -rf "$EVIDENCE_DIR"
    install -d -m 0700 "$EVIDENCE_DIR"
    cp -a "$ARTIFACT_DIR/." "$EVIDENCE_DIR/"
  fi

  # agent-terminal derives each scope's socket dir as
  # /tmp/agent-terminal-<scope-digest> (src/paths.rs) and creates the scope
  # root under this run's state dir ($STATE_DIR/scopes/<digest>) on first
  # bootstrap. Clean up ONLY this run's derived socket dirs by reading the
  # digests back from our own state root — never by globbing
  # /tmp/agent-terminal-* or killing sessions by global wildcard (other runs'
  # socket dirs would be swept). Zellij servers in those dirs are killed
  # first so no detached session survives; the dirs are then removed. The
  # bundled zellij is used: a host zellij is not required, and only the
  # bundled server can exist in this run's socket dirs.
  if [[ -d $STATE_DIR/scopes && -n ${BUNDLED_ZELLIJ:-} && -x $BUNDLED_ZELLIJ ]]; then
    for scope_root in "$STATE_DIR"/scopes/*; do
      [[ -d $scope_root ]] || continue
      digest="$(basename "$scope_root")"
      at_sock="/tmp/agent-terminal-$digest"
      [[ -d $at_sock ]] || continue
      sessions="$(env ZELLIJ_SOCKET_DIR="$at_sock" "$BUNDLED_ZELLIJ" list-sessions --short --no-formatting 2>/dev/null || true)"
      while IFS= read -r session; do
        if [[ $session == agent-terminal-* ]]; then
          env ZELLIJ_SOCKET_DIR="$at_sock" "$BUNDLED_ZELLIJ" kill-session "$session" >/dev/null 2>&1 || true
        fi
      done <<<"$sessions"
      rm -rf "$at_sock" 2>/dev/null || true
    done
  fi

  rm -rf "$SANDBOX"
  if [[ -n ${EVIDENCE_DIR:-} && -d $EVIDENCE_DIR ]]; then
    printf 'Evidence retained at %s\n' "$EVIDENCE_DIR"
  fi

  exit "$exit_status"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

case "$AGENT_TERMINAL_AGENT" in
  pi|omp) ;;
  *) fail "AGENT_TERMINAL_AGENT must be pi or omp, got: $AGENT_TERMINAL_AGENT" ;;
esac

# The hold must be long enough for the mid-lifecycle cross-checks below.
if [[ ! $FIXTURE_HOLD_SECS =~ ^[0-9]+$ ]] || (( FIXTURE_HOLD_SECS < 30 )); then
  fail "AGENT_TERMINAL_FIXTURE_HOLD_SECS must be an integer >= 30, got: $FIXTURE_HOLD_SECS"
fi

missing=()
for tool in bun curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "missing required tools: ${missing[*]}"
fi

# The agent-terminal binary under test. Defaults to the musl release build
# (what the npm bundle packages); AGENT_TERMINAL_BIN overrides it. An
# explicit override is validated up front; the default is validated only
# after the preflight build below has had a chance to produce it (and is
# skipped entirely with AGENT_TERMINAL_SKIP_PREFLIGHT=1, in which case the
# pre-existing binary must be valid).
if [[ -n ${AGENT_TERMINAL_BIN:-} ]]; then
  AT_BIN="$AGENT_TERMINAL_BIN"
  if [[ ! -x $AT_BIN ]]; then
    fail "agent-terminal binary not found: $AT_BIN (build it or set AGENT_TERMINAL_BIN)"
  fi
else
  AT_BIN="$REPO_ROOT/target/x86_64-unknown-linux-musl/release/agent-terminal"
fi

# The agent wrapper + the bundle for the agent under test. The build produces
# the dist/index.js extension and the bundled bin/zellij used by the harness
# PATH below.
if [[ $AGENT_TERMINAL_AGENT == pi ]]; then
  WS_ROOT="$REPO_ROOT/pi"
  BUNDLE_PACKAGE="$WS_ROOT/npm/packages/pi-agent-terminal-bundle-zellij"
  PI_DIR="${AGENT_TERMINAL_PI_DIR:-$HOME/.local/pi/pkg}"
  if [[ ! -f $PI_DIR/dist/cli.js ]]; then
    fail "pi package not found at $PI_DIR (set AGENT_TERMINAL_PI_DIR)"
  fi
else
  WS_ROOT="$REPO_ROOT/omp"
  BUNDLE_PACKAGE="$WS_ROOT/npm/packages/omp-agent-terminal-bundle-zellij"
  OMP_BIN="$(command -v omp 2>/dev/null || true)"
  if [[ -z $OMP_BIN && -x /usr/local/bin/omp ]]; then
    OMP_BIN=/usr/local/bin/omp
  fi
  if [[ -z $OMP_BIN ]]; then
    fail "omp binary not found on PATH or at /usr/local/bin/omp"
  fi
fi

if [[ $SKIP_PREFLIGHT != 1 ]]; then
  printf 'Building %s bundle ...\n' "$AGENT_TERMINAL_AGENT"
  cd "$WS_ROOT"
  if ! bun run build; then
    fail "bun run build failed for $WS_ROOT"
  fi
fi

EXTENSION_PATH="$BUNDLE_PACKAGE/dist/index.js"
if [[ ! -f $EXTENSION_PATH ]]; then
  fail "extension dist missing after build: $EXTENSION_PATH"
fi
BUNDLED_ZELLIJ_DIR="$BUNDLE_PACKAGE/bin/zellij"
BUNDLED_ZELLIJ="$BUNDLED_ZELLIJ_DIR/zellij"
if [[ ! -x $BUNDLED_ZELLIJ ]]; then
  fail "bundled zellij missing after build: $BUNDLED_ZELLIJ"
fi

# The default binary is validated only now: the preflight build above (or a
# previously built artifact when SKIP_PREFLIGHT=1) is what produces it.
if [[ -z ${AGENT_TERMINAL_BIN:-} && ! -x $AT_BIN ]]; then
  fail "agent-terminal binary not found: $AT_BIN (build it or set AGENT_TERMINAL_BIN)"
fi
# The harness resolves agent-terminal from the bundle's
# bin/linux-x64/agent-terminal (pi/npm/src/index.ts, omp/npm/src/index.ts),
# so the installed copy must be the binary under test: installing after the
# build keeps the agent-side cross-checks and the host-side cross-checks on
# the exact same binary.
BUNDLE_AT_BIN="$BUNDLE_PACKAGE/bin/linux-x64/agent-terminal"
install -m 0755 "$AT_BIN" "$BUNDLE_AT_BIN"

install -d -m 0700 \
  "$SANDBOX" \
  "$WORKDIR" \
  "$CONFIG_DIR" \
  "$DATA_DIR" \
  "$CACHE_DIR" \
  "$STATE_DIR" \
  "$SOCKET_DIR" \
  "$SANDBOX/home" \
  "$ARTIFACT_DIR" \
  "$SANDBOX/bin"

# The agent wrapper. pi runs as `bun <package>/dist/cli.js`; omp is a
# standalone binary. Both are wrapped so the harness can point at them
# uniformly.
AGENT_BIN="$SANDBOX/bin/$AGENT_TERMINAL_AGENT"
if [[ $AGENT_TERMINAL_AGENT == pi ]]; then
  cat >"$AGENT_BIN" <<EOF
#!/usr/bin/env bash
exec bun "$PI_DIR/dist/cli.js" "\$@"
EOF
else
  cat >"$AGENT_BIN" <<EOF
#!/usr/bin/env bash
exec "$OMP_BIN" "\$@"
EOF
fi
chmod 0755 "$AGENT_BIN"

# The provider extension registers the fixture provider (one file per
# fixture: the BASE_URL differs). The ports are not known until the fixtures
# have bound them, so the extensions are generated after fixture startup
# below.
provider_extension() {
  local port="$1"
  local out="$2"
  cat >"$out" <<EOF
export default (api: any) => {
  api.registerProvider("local-fixture", {
    baseUrl: "http://127.0.0.1:$port/v1",
    apiKey: "test",
    api: "openai-completions",
    models: [
      {
        id: "fixture",
        name: "local-fixture/fixture",
        reasoning: false,
        input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        contextWindow: 16384,
        maxTokens: 4096,
      },
    ],
  })
}
EOF
  chmod 0600 "$out"
}
PROVIDER_A="$SANDBOX/provider-a.ts"
PROVIDER_B="$SANDBOX/provider-b.ts"

# The two-session prompt drives the same lifecycle as e2e-pi.sh's prompt,
# with the hold step inserted between start and the first read. The probe
# steps differ per agent (pi: PI_SESSION_ID; omp: injected
# AGENT_TERMINAL_SCOPE plus an explicit override probe), so the prompt's step
# count and numbering shift accordingly: 10 lifecycle steps are written once
# with __STEP_N__ markers and renumbered below.
if [[ $AGENT_TERMINAL_AGENT == omp ]]; then
  TOTAL_STEPS=12
  LIFECYCLE_OFFSET=2
  # Scope shell reference for the start-command pane marker (scope-marker:):
  # the fixture exact-matches the start command, and the marker must expand
  # to the scope of the session that spawned the pane. omp exposes it via the
  # extension-injected AGENT_TERMINAL_SCOPE; pi via PI_SESSION_ID. The
  # single-quoted value is substituted verbatim into the prompt text (the
  # agent types it into the Bash tool, the job shell expands it).
  SCOPE_MARKER_ENV='$AGENT_TERMINAL_SCOPE'
else
  TOTAL_STEPS=11
  LIFECYCLE_OFFSET=1
  SCOPE_MARKER_ENV='$PI_SESSION_ID'
fi

readonly PROMPT_FILE="$SANDBOX/prompt.md"
{
  cat <<'PROMPT_HEAD'
This is an execution task, not a writing or planning task. Work only in __WORKDIR__. Do not output any prose, Markdown, YAML, code block, template, plan, or simulated transcript.

Execution contract:
- The ONLY tool you may use is Bash. Every assistant turn must emit exactly one Bash tool call for the first incomplete numbered step below.
- Wait for the real Bash command's JSON output, inspect it, and then emit the next Bash tool call. Do not predict or invent command output.
- You must NOT call the read, write, task, or any other non-Bash tool for any purpose. The word "read" below always means the Bash command `agent-terminal read`, never the read tool.
- You must NOT delegate to a subagent, create files, or emit simulated/agent-terminal commands inside Markdown or YAML blocks.
- If any step fails, stop and report the failure; only end with E2E_SUCCESS after step __TOTAL_STEPS__ passes.

Perform this exact __TOTAL_STEPS__-step lifecycle using the job name __JOB_NAME__:
PROMPT_HEAD
  if [[ $AGENT_TERMINAL_AGENT == omp ]]; then
    cat <<'PROMPT_PROBE'
1. Bash: run `printenv AGENT_TERMINAL_SCOPE` and confirm it prints the current omp session id (non-empty). This verifies the extension injected the per-session scope into the Bash tool environment.
2. Bash: run `AGENT_TERMINAL_SCOPE=shared printenv AGENT_TERMINAL_SCOPE` and confirm it prints the literal text shared. This verifies an explicit scope override wins over the injected default.
PROMPT_PROBE
  else
    cat <<'PROMPT_PROBE'
1. Bash: run `printenv PI_SESSION_ID` and confirm it prints the current pi session id (non-empty). This verifies the per-session scope is available to the Bash tool.
PROMPT_PROBE
  fi
  cat <<'PROMPT_LIFECYCLE'
__STEP_1__. Bash: `agent-terminal list` and verify the JSON response has status ok and an empty jobs array.
__STEP_2__. Bash: start the interactive job with `agent-terminal start __JOB_NAME__ -- /bin/bash -lc 'printf "prompt-ready\n"; printf "scope-marker:%s\n" "__SCOPE_MARKER_ENV__"; IFS= read -r first; printf "first:%s\n" "$first"; IFS= read -r second; printf "second:%s\n" "$second"'`.
__STEP_3__. Bash: `sleep __HOLD_SECS__` (wait for the other session's hold to overlap; do not skip this step).
__STEP_4__. Bash: `agent-terminal read __JOB_NAME__` and verify its JSON screen contains prompt-ready. Retry the Bash read command briefly only if the pane has not rendered it yet.
__STEP_5__. Bash: send the literal text hello-e2e with `agent-terminal send __JOB_NAME__ -- hello-e2e`.
__STEP_6__. Bash: `agent-terminal read __JOB_NAME__` and verify its JSON screen contains first:hello-e2e.
__STEP_7__. Bash: press Enter with `agent-terminal press __JOB_NAME__ -- Enter`.
__STEP_8__. Bash: `agent-terminal read __JOB_NAME__` and verify the JSON reports state exited with exit_code 0 and its screen contains second:.
__STEP_9__. Bash: `agent-terminal stop __JOB_NAME__` and verify the JSON response is the status ok acknowledgement.
__STEP_10__. Bash: `agent-terminal list` and verify the JSON response has status ok and an empty jobs array.

Do not modify repository files. After all __TOTAL_STEPS__ steps pass, end your final response with a separate line containing exactly E2E_SUCCESS. Do not print E2E_SUCCESS if any step fails.
PROMPT_LIFECYCLE
} >"$PROMPT_FILE"

sed_exprs=(-e "s/__TOTAL_STEPS__/$TOTAL_STEPS/g" -e "s/__JOB_NAME__/$JOB_NAME/g" -e "s|__WORKDIR__|$WORKDIR|g" -e "s/__HOLD_SECS__/$FIXTURE_HOLD_SECS/g" -e "s/__SCOPE_MARKER_ENV__/$SCOPE_MARKER_ENV/g")
for i in $(seq 1 10); do
  sed_exprs+=(-e "s/__STEP_${i}__/$((i + LIFECYCLE_OFFSET))/g")
done
sed -i "${sed_exprs[@]}" "$PROMPT_FILE"

# Start both fixture servers on ephemeral ports (--port 0): the kernel
# assigns each a distinct port atomically — no free-port probing, no TOCTOU
# race — and each fixture reports its actual port as FIXTURE_PORT=<digits>
# on stdout. They read AGENT_TERMINAL_AGENT and AGENT_TERMINAL_JOB_NAME at
# process start to select the probe steps and the exact job name.
export AGENT_TERMINAL_AGENT
export AGENT_TERMINAL_JOB_NAME="$JOB_NAME"
export AGENT_TERMINAL_FIXTURE_HOLD=1
export AGENT_TERMINAL_FIXTURE_HOLD_SECS="$FIXTURE_HOLD_SECS"

printf 'Starting fixture A on an ephemeral port ...\n'
bun run "$FIXTURE_SCRIPT" --port 0 >"$ARTIFACT_DIR/fixture-a.log" 2>&1 &
FIXTURE_A_PID=$!
printf 'Starting fixture B on an ephemeral port ...\n'
bun run "$FIXTURE_SCRIPT" --port 0 >"$ARTIFACT_DIR/fixture-b.log" 2>&1 &
FIXTURE_B_PID=$!

# Read each fixture's actual port back from its log.
fixture_port_of() {
  local label="$1"
  local pid="$2"
  local log="$ARTIFACT_DIR/fixture-$label.log"
  local port=""
  for _ in $(seq 1 30); do
    if ! kill -0 "$pid" 2>/dev/null; then
      cat "$log" >&2
      fail "fixture $label exited before reporting its port (see $log)"
    fi
    port="$(grep -m1 -o 'FIXTURE_PORT=[0-9][0-9]*' "$log" 2>/dev/null | cut -d= -f2 || true)"
    if [[ -n $port ]]; then
      printf '%s' "$port"
      return 0
    fi
    sleep 0.2
  done
  cat "$log" >&2
  fail "fixture $label never reported its port (FIXTURE_PORT=<digits> missing from $log)"
}
FIXTURE_PORT_A="$(fixture_port_of a "$FIXTURE_A_PID")"
FIXTURE_PORT_B="$(fixture_port_of b "$FIXTURE_B_PID")"
if [[ -z $FIXTURE_PORT_A || -z $FIXTURE_PORT_B || $FIXTURE_PORT_A == "$FIXTURE_PORT_B" ]]; then
  fail "the two fixtures bound the same port: '$FIXTURE_PORT_A'"
fi

# The provider extension registers the fixture provider (one file per
# fixture: the BASE_URL differs).
provider_extension "$FIXTURE_PORT_A" "$PROVIDER_A"
provider_extension "$FIXTURE_PORT_B" "$PROVIDER_B"

wait_for_fixture() {
  local port="$1"
  local pid="$2"
  local label="$3"
  for _ in $(seq 1 30); do
    if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
      return 0
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
      fail "fixture $label exited before becoming healthy (see $ARTIFACT_DIR/fixture-$label.log)"
    fi
    sleep 0.2
  done
  fail "fixture $label did not become healthy within 30 checks"
}
wait_for_fixture "$FIXTURE_PORT_A" "$FIXTURE_A_PID" a
wait_for_fixture "$FIXTURE_PORT_B" "$FIXTURE_B_PID" b

# Launch both agents in parallel. Each runs in a pristine env -i sandbox whose
# AGENT_TERMINAL_STATE/HOME are shared by its Bash children (both agents
# export the launch env to the Bash tool; verified empirically for omp and pi
# alike). The agents are launched from the workdir so the Bash tool's default
# cwd is the workdir. AGENT_TERMINAL_SCOPE is deliberately NOT set: the scope
# must come from the agent's own session id (pi's native PI_SESSION_ID, omp's
# injected AGENT_TERMINAL_SCOPE).
AGENT_FLAGS=""
if [[ $AGENT_TERMINAL_AGENT == omp ]]; then
  AGENT_FLAGS="--auto-approve --no-pty"
fi

launch_agent() {
  local id="$1"
  local provider_ext="$2"
  local transcript="$ARTIFACT_DIR/session-$id.jsonl"
  (
    cd "$WORKDIR"
    # AGENT_FLAGS is intentionally word-split so omp can add its native flags.
    # shellcheck disable=SC2086
    exec env -i \
      HOME="$SANDBOX/home" \
      USER="$(id -un)" \
      LOGNAME="$(id -un)" \
      SHELL=/bin/bash \
      PATH="$PATH" \
      LANG=C.UTF-8 \
      TERM=xterm-256color \
      XDG_CONFIG_HOME="$CONFIG_DIR" \
      XDG_DATA_HOME="$DATA_DIR" \
      XDG_CACHE_HOME="$CACHE_DIR" \
      XDG_STATE_HOME="$STATE_DIR" \
      PI_CODING_AGENT_DIR="$CONFIG_DIR" \
      PI_OFFLINE=1 \
      AGENT_TERMINAL_STATE="$STATE_DIR" \
      ZELLIJ_SOCKET_DIR="$SOCKET_DIR" \
      timeout "$PROMPT_E2E_TIMEOUT" "$AGENT_BIN" \
      -p --mode json --model local-fixture/fixture \
      -e "$EXTENSION_PATH" -e "$provider_ext" \
      --no-skills --no-context-files --no-lsp --no-session --thinking off \
      --cwd "$WORKDIR" $AGENT_FLAGS "$(cat "$PROMPT_FILE")"
  ) >"$transcript" 2>"$ARTIFACT_DIR/session-$id.stderr.log" &
  echo "$!" >"$ARTIFACT_DIR/session-$id.pid"
}

launch_agent a "$PROVIDER_A"
launch_agent b "$PROVIDER_B"
AGENT_PIDS[a]="$(<"$ARTIFACT_DIR/session-a.pid")"
AGENT_PIDS[b]="$(<"$ARTIFACT_DIR/session-b.pid")"

# Wait until BOTH sessions have started their job (the `start server` tool
# call). The fixture fails loudly on any lifecycle error, which surfaces as
# E2E_FIXTURE_ERROR in the transcript.
phase "waiting for both sessions to start"
for id in a b; do
  transcript="$ARTIFACT_DIR/session-$id.jsonl"
  for _ in $(seq 1 900); do
    if [[ -f $transcript ]] && grep -q 'agent-terminal start server' "$transcript" 2>/dev/null; then
      break
    fi
    if ! kill -0 "${AGENT_PIDS[$id]}" 2>/dev/null; then
      fail "agent session $id exited before starting the job (see $ARTIFACT_DIR/session-$id.stderr.log)"
    fi
    if grep -q 'E2E_FIXTURE_ERROR' "$transcript" 2>/dev/null; then
      fail "fixture reported an error in session $id"
    fi
    sleep 0.5
  done
  if ! grep -q 'agent-terminal start server' "$transcript" 2>/dev/null; then
    fail "agent session $id never started the job within the timeout"
  fi
done

# The first line of --mode json output is the session header carrying `id`.
session_id_of() {
  local transcript="$1"
  SESSION_ID="$(bun -e '
    const [line] = (await Bun.file(process.argv[1]).text()).split("\n")
    if (!line) process.exit(2)
    const parsed = JSON.parse(line)
    if (parsed.type !== "session" || typeof parsed.id !== "string") process.exit(3)
    console.log(parsed.id)
  ' "$transcript" 2>/dev/null || true)"
  if [[ -z $SESSION_ID ]]; then
    fail "could not parse the session id from $transcript"
  fi
}

# Cross-check A and B: distinct session ids prove the two agents are distinct
# sessions.
phase "checking session isolation"
session_id_of "$ARTIFACT_DIR/session-a.jsonl"
SESSION_A="$SESSION_ID"
session_id_of "$ARTIFACT_DIR/session-b.jsonl"
SESSION_B="$SESSION_ID"
if [[ -z $SESSION_A || -z $SESSION_B || $SESSION_A == "$SESSION_B" ]]; then
  fail "the two sessions share a session id: '$SESSION_A' vs '$SESSION_B'"
fi
{
  printf 'session A id: %s\n' "$SESSION_A"
  printf 'session B id: %s\n' "$SESSION_B"
} | tee -a "$ARTIFACT_DIR/isolation.log"

# While both jobs are held (sleep step), run the host-side scope
# cross-checks. The agent-terminal CLI resolves the scope from
# AGENT_TERMINAL_SCOPE first, then PI_SESSION_ID, then "standalone"; the
# state root (and therefore the Zellij socket dir) is derived from the scope
# digest under AGENT_TERMINAL_STATE, which the cross-check shares with the
# agents so it sees the same scope roots. The bundled Zellij is provided via
# PATH so the CLI's internal `zellij` invocation resolves. ZELLIJ_SOCKET_DIR
# is passed (with the same shared value the agents get) although the CLI
# ignores it: per-scope namespaces are derived by src/paths.rs under the OS
# temp dir, and the cross-check's socket assertions target exactly those
# derived dirs.
run_scope_cli() {
  local scope="$1"
  shift
  # The CLI derives the project from the invocation directory (find_project_root),
  # so cross-checks MUST run from the shared workdir: the agents' Bash children
  # run there, and state is keyed by (project digest, scope digest).
  (
    cd "$WORKDIR"
    PATH="$BUNDLED_ZELLIJ_DIR:$PATH" \
      AGENT_TERMINAL_STATE="$STATE_DIR" AGENT_TERMINAL_SCOPE="$scope" \
      ZELLIJ_SOCKET_DIR="$SOCKET_DIR" \
      "$AT_BIN" "$@" 2>"$ARTIFACT_DIR/scope-cli.err.log"
  )
}

scope_tag() {
  local scope="$1"
  # Scope ids may contain characters that are awkward in filenames.
  printf '%s' "$scope" | tr -c 'A-Za-z0-9._-' '_'
}

# The start marker can appear in the transcript before the tool result lands;
# poll each scope's list until the job is running.
phase "cross-checking each scope's job list"
for scope in "$SESSION_A" "$SESSION_B"; do
  tag="$(scope_tag "$scope")"
  seen=false
  for _ in $(seq 1 60); do
    # The start marker can precede the tool result; while the agent's own
    # `agent-terminal start` is still in flight the controller lock is held
    # and list fails transiently ("another controller operation is in
    # progress"). Treat that as not-yet and keep polling.
    if ! run_scope_cli "$scope" list >"$ARTIFACT_DIR/scope-list-$tag.json"; then
      cat "$ARTIFACT_DIR/scope-cli.err.log" >"$ARTIFACT_DIR/scope-list-$tag.check.log"
      sleep 0.5
      continue
    fi
    if EXPECTED_JOB="$JOB_NAME" JSON_FILE="$ARTIFACT_DIR/scope-list-$tag.json" bun -e '
      const body = await Bun.file(process.env.JSON_FILE).json()
      if (typeof body !== "object" || body === null || Array.isArray(body) || body.status !== "ok") {
        throw new Error(`unexpected response: ${JSON.stringify(body)}`)
      }
      if (!Array.isArray(body.jobs) || body.jobs.length !== 1) {
        throw new Error(`expected exactly one job in scope, got ${JSON.stringify(body.jobs)}`)
      }
      const job = body.jobs[0]
      if (typeof job !== "object" || job === null || job.job !== process.env.EXPECTED_JOB || job.state !== "running") {
        throw new Error(`unexpected job entry: ${JSON.stringify(job)}`)
      }
    ' 2>"$ARTIFACT_DIR/scope-list-$tag.check.log"; then
      seen=true
      break
    fi
    sleep 0.5
  done
  if [[ $seen != true ]]; then
    cat "$ARTIFACT_DIR/scope-list-$tag.check.log" >&2
    fail "scope $tag never showed exactly one running job named $JOB_NAME"
  fi
done

phase "cross-checking the held job's screen"
# Each job's pane prints a scope marker (`scope-marker:<scope>`) right after
# prompt-ready, so the held panes of the two identically named jobs are
# distinguishable: read must report state running, the queried scope's marker,
# and NO marker of the peer scope. Reads are polled like the list check
# above: while the agent's own controller operation is still in flight the
# per-scope state lock is busy ("another controller operation is in
# progress", exit 1), which is transient and retried; permanent failures
# (job vanished, non-JSON output, assertion failures) fail immediately.
for scope in "$SESSION_A" "$SESSION_B"; do
  tag="$(scope_tag "$scope")"
  peer_scope="$(if [[ $scope == "$SESSION_A" ]]; then printf '%s' "$SESSION_B"; else printf '%s' "$SESSION_A"; fi)"
  seen=false
  for _ in $(seq 1 60); do
    if ! run_scope_cli "$scope" read "$JOB_NAME" >"$ARTIFACT_DIR/scope-read-$tag.json"; then
      # The CLI always reports errors as a JSON response on stdout with a
      # nonzero exit. A busy controller lock (exit 1, code lock_busy) is
      # transient while the agent's own operation is in flight: retry it.
      # Any other error (job vanished, zellij failure) is permanent: fail.
      if grep -q '"code":"lock_busy"' "$ARTIFACT_DIR/scope-read-$tag.json" 2>/dev/null; then
        sleep 0.5
        continue
      fi
      cat "$ARTIFACT_DIR/scope-read-$tag.json" >&2
      cat "$ARTIFACT_DIR/scope-cli.err.log" >&2
      fail "agent-terminal read failed for scope $tag"
    fi
    if SCOPE="$scope" PEER_SCOPE="$peer_scope" JSON_FILE="$ARTIFACT_DIR/scope-read-$tag.json" bun -e '
      const body = await Bun.file(process.env.JSON_FILE).json()
      if (typeof body !== "object" || body === null || Array.isArray(body) || body.status !== "ok") {
        throw new Error(`unexpected read response: ${JSON.stringify(body)}`)
      }
      if (body.state !== "running") {
        throw new Error(`expected state running, got ${JSON.stringify(body.state)}`)
      }
      if (typeof body.screen !== "string" || !body.screen.includes("prompt-ready")) {
        throw new Error(`screen lacks prompt-ready: ${JSON.stringify(body.screen)}`)
      }
      if (!body.screen.includes(`scope-marker:${process.env.SCOPE}`)) {
        throw new Error(`screen lacks own scope marker scope-marker:${process.env.SCOPE}: ${JSON.stringify(body.screen)}`)
      }
      if (body.screen.includes(`scope-marker:${process.env.PEER_SCOPE}`)) {
        throw new Error(`screen leaks the peer scope marker scope-marker:${process.env.PEER_SCOPE}`)
      }
    ' 2>"$ARTIFACT_DIR/scope-read-$tag.check.log"; then
      seen=true
      break
    fi
    sleep 0.5
  done
  if [[ $seen != true ]]; then
    cat "$ARTIFACT_DIR/scope-read-$tag.check.log" >&2
    fail "scope $tag never showed a running held pane with its own scope marker"
  fi
done

# Both sessions complete their lifecycle independently; the fixture fails the
# run if the job disappeared mid-lifecycle or if a second job appeared in the
# final list.
phase "waiting for both sessions to finish"
for id in a b; do
  wait_status=0
  wait "${AGENT_PIDS[$id]}" || wait_status=$?
  # The session was reaped; drop it from the trap's kill set BEFORE any
  # diagnostics, so the EXIT cleanup (or a signal during it) can never
  # signal a PID the kernel may already have recycled.
  unset 'AGENT_PIDS['$id']'
  if (( wait_status != 0 )); then
    cat "$ARTIFACT_DIR/session-$id.stderr.log" >&2
    cat "$ARTIFACT_DIR/session-$id.jsonl" >&2
    fail "agent session $id failed"
  fi
done

# Final transcript assertions. The transcript is parsed as JSONL, never
# grepped: Bash tool calls and their results are matched in the order they
# STARTED (tool_execution_start order — the fixture emits one bash call per
# step, so start order is the lifecycle order), the probe prefix and the full
# pi/omp lifecycle must appear in order (read steps may retry with the same
# command, exactly like the fixture), E2E_SUCCESS must be a standalone line
# in the final assistant message AFTER the last tool result, and
# E2E_FIXTURE_ERROR anywhere fails the run.
phase "verifying transcripts"
for id in a b; do
  transcript="$ARTIFACT_DIR/session-$id.jsonl"
  if ! JSON_FILE="$transcript" AGENT="$AGENT_TERMINAL_AGENT" JOB_NAME="$JOB_NAME" HOLD_SECS="$FIXTURE_HOLD_SECS" bun -e '
    const lines = (await Bun.file(process.env.JSON_FILE).text()).split("\n").filter(Boolean)
    const agent = process.env.AGENT
    const job = process.env.JOB_NAME
    // parseInt matches the fixture HOLD_SECS normalization exactly.
    const hold = String(parseInt(process.env.HOLD_SECS, 10))
    if (agent !== "pi" && agent !== "omp") throw new Error(`unexpected agent: ${agent}`)
    if (job === "" || Number.isNaN(parseInt(process.env.HOLD_SECS, 10))) {
      throw new Error("missing JOB_NAME/HOLD_SECS")
    }

    // The pane marker env var, matching the SCOPE_MARKER_ENV used by
    // e2e-fixture.ts and the scopeMarkerEnv of e2e-verify.ts byte-for-byte.
    const scopeEnv = agent === "omp" ? "AGENT_TERMINAL_SCOPE" : "PI_SESSION_ID"
    const start = `agent-terminal start ${job} -- /bin/bash -lc '"'"'printf "prompt-ready\\n"; printf "scope-marker:%s\\n" "$${scopeEnv}"; IFS= read -r first; printf "first:%s\\n" "$first"; IFS= read -r second; printf "second:%s\\n" "$second"'"'"'`
    const probes =
      agent === "omp"
        ? ["printenv AGENT_TERMINAL_SCOPE", "AGENT_TERMINAL_SCOPE=shared printenv AGENT_TERMINAL_SCOPE"]
        : ["printenv PI_SESSION_ID"]
    // The expected lifecycle, in order, with the same per-step checks as the
    // fixture and e2e-verify.ts. Only steps whose check may legitimately fail
    // transiently (the three reads: the pane renders asynchronously) retry
    // with the identical command.
    const steps = [
      {
        cmd: "agent-terminal list",
        check: (b) => Array.isArray(b.jobs) && b.jobs.length === 0,
      },
      {
        cmd: start,
        check: (b) => b.state === "running",
      },
      { cmd: `sleep ${hold}`, hold: true },
      {
        cmd: `agent-terminal read ${job}`,
        check: (b) => typeof b.screen === "string" && b.screen.includes("prompt-ready"),
      },
      { cmd: `agent-terminal send ${job} -- hello-e2e`, check: () => true },
      {
        cmd: `agent-terminal read ${job}`,
        check: (b) => typeof b.screen === "string" && b.screen.includes("first:hello-e2e"),
      },
      { cmd: `agent-terminal press ${job} -- Enter`, check: () => true },
      {
        cmd: `agent-terminal read ${job}`,
        check: (b) => b.state === "exited" && b.exit_code === 0 && typeof b.screen === "string" && b.screen.includes("second:"),
      },
      { cmd: `agent-terminal stop ${job}`, check: () => true },
      {
        cmd: "agent-terminal list",
        check: (b) => Array.isArray(b.jobs) && b.jobs.length === 0,
      },
    ]

    // Collect completed bash tool calls in START order.
    const pending = new Map()
    const calls = []
    for (const line of lines) {
      const ev = JSON.parse(line)
      if (ev.type === "tool_execution_start" && ev.toolName === "bash" && typeof ev.args?.command === "string") {
        pending.set(ev.toolCallId, ev.args.command)
      } else if (ev.type === "tool_execution_end" && pending.has(ev.toolCallId)) {
        calls.push({ command: pending.get(ev.toolCallId), result: ev.result })
        pending.delete(ev.toolCallId)
      }
    }
    if (pending.size > 0) {
      throw new Error(`a bash tool call did not complete: ${Array.from(pending.values()).join(", ")}`)
    }

    function outputText(result) {
      const content = result && typeof result === "object" && Array.isArray(result.content) ? result.content : []
      for (const part of content) {
        if (part && typeof part === "object" && part.type === "text" && typeof part.text === "string") return part.text
      }
      return ""
    }
    // omp bash appends "\n\nWall time: X seconds\n\nCommand exited with code N".
    // Strip the suffix BEFORE trimming, exactly like the fixture and the
    // verifier: a probe that printed nothing but the Wall-time trailer must
    // count as empty.
    function stripped(text) {
      return text.replace(/\n\nWall time:.*$/s, "").trim()
    }
    // Extract the first JSON object from the output, requiring status ok.
    function parseOk(text) {
      const trimmed = stripped(text)
      const jsonStart = trimmed.indexOf("{")
      if (jsonStart < 0) return null
      for (let end = jsonStart + 1; end <= trimmed.length; end++) {
        try {
          const body = JSON.parse(trimmed.slice(jsonStart, end))
          if (typeof body === "object" && body !== null && body.status === "ok") return body
        } catch {}
      }
      return null
    }

    // The scope probes must be the first bash calls, in order.
    let i = 0
    for (const probe of probes) {
      const call = calls[i]
      if (call === undefined || call.command.trim() !== probe) {
        throw new Error(`expected probe bash call "${probe}", got ${call === undefined ? "nothing" : JSON.stringify(call.command)}`)
      }
      if (stripped(outputText(call.result)) === "") {
        throw new Error(`probe "${probe}" printed nothing`)
      }
      i++
    }

    // The lifecycle in order, with the same per-step checks and retry policy
    // as e2e-verify.ts: a read step whose output is successful JSON but does
    // not yet satisfy the step check may retry with the identical command
    // (the pane renders asynchronously); any non-ok output is a permanent
    // failure. Attempts are counted per step and capped at 8, matching the
    // verifier real-mode cap.
    let stepIdx = 0
    let attempts = 0
    while (i < calls.length) {
      const step = steps[stepIdx]
      if (step === undefined) {
        throw new Error(`extra bash call after the lifecycle: ${calls[i].command}`)
      }
      const call = calls[i]
      if (call.command.trim() !== step.cmd) {
        throw new Error(`out-of-order bash call at lifecycle step ${stepIdx + 1}: expected "${step.cmd}", got "${call.command.trim()}"`)
      }
      i++
      if (step.hold) {
        // The sleep step output is irrelevant, exactly like the fixture.
        stepIdx++
        attempts = 0
        continue
      }
      const body = parseOk(outputText(call.result))
      if (body === null) {
        throw new Error(`lifecycle step ${stepIdx + 1} ("${step.cmd}") result is not an ok status response`)
      }
      if (step.check(body)) {
        stepIdx++
        attempts = 0
        continue
      }
      attempts++
      if (attempts >= 8) {
        throw new Error(`lifecycle step ${stepIdx + 1} ("${step.cmd}") failed ${attempts} attempts`)
      }
    }
    if (stepIdx < steps.length) {
      const missing = steps.slice(stepIdx).map((s) => s.cmd).join(", ")
      throw new Error(`lifecycle incomplete; missing: ${missing}`)
    }

    // E2E_FIXTURE_ERROR must never appear.
    for (const line of lines) {
      if (line.includes("E2E_FIXTURE_ERROR")) {
        throw new Error("E2E_FIXTURE_ERROR found in transcript")
      }
    }

    // E2E_SUCCESS must be a standalone line in the final assistant message,
    // after the last tool result.
    let lastToolEnd = -1
    let lastAssistantEnd = -1
    let lastAssistantHasSuccess = false
    for (let idx = 0; idx < lines.length; idx++) {
      const ev = JSON.parse(lines[idx])
      if (ev.type === "tool_execution_end") lastToolEnd = idx
      if (ev.type !== "message_end") continue
      const message = ev.message
      if (message === null || typeof message !== "object" || message.role !== "assistant") continue
      const content = message.content
      const text = typeof content === "string"
        ? content
        : Array.isArray(content)
          ? content.filter((p) => p && typeof p === "object" && p.type === "text").map((p) => p.text).join("\n")
          : ""
      if (text.trim() === "") continue
      lastAssistantEnd = idx
      lastAssistantHasSuccess = text.split(/\r?\n/).some((l) => l.trim() === "E2E_SUCCESS")
    }
    if (lastAssistantEnd <= lastToolEnd || !lastAssistantHasSuccess) {
      throw new Error("no standalone E2E_SUCCESS line in the final assistant message after the last tool result")
    }

    // The final list of EACH session must be empty: with a shared state root,
    // only the scope digest separates the two sessions, so a leaked job in
    // the other scope would show up here. The final bash call is the final
    // `agent-terminal list` (the matcher above consumed every call in order).
    const finalCall = calls[calls.length - 1]
    if (finalCall === undefined || finalCall.command.trim() !== "agent-terminal list") {
      throw new Error("final bash call is not the final agent-terminal list")
    }
    const body = JSON.parse(stripped(outputText(finalCall.result)))
    if (!Array.isArray(body.jobs) || body.jobs.length !== 0) {
      throw new Error(`final list was not empty: ${JSON.stringify(body)}`)
    }
  ' 2>"$ARTIFACT_DIR/transcript-$id.check.log"; then
    cat "$ARTIFACT_DIR/transcript-$id.check.log" >&2
    fail "session $id transcript verification failed"
  fi
done

printf 'Two-session isolation e2e passed.\n'
