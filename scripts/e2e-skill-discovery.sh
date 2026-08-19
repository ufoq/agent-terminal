#!/usr/bin/env bash
#
# Skill-discovery e2e gate for agent-terminal through the pi and omp coding
# agents.
#
# Starts a local OpenAI-compatible fixture server in skill-probe mode
# (AGENT_TERMINAL_FIXTURE_SKILL_PROBE=1) and runs the agent under test with a
# trivial prompt and skills ENABLED (no --no-skills). The fixture inspects the
# FIRST provider request and asserts that the agent-terminal skill description
# ("Run persistent or interactive terminal jobs through a simple Zellij
# wrapper") reached the model verbatim:
#   - found    -> replies SKILL_PROBE_OK and stops the session;
#   - not found -> replies SKILL_PROBE_FAILED and stops the session.
# The gate passes only when the transcript contains SKILL_PROBE_OK.
#
# This proves the skill packaging contract for each agent: the bundle package
# ROOT (pi: npm/packages/pi-agent-terminal-bundle-zellij; omp:
# npm/packages/omp-agent-terminal-bundle-zellij) ships the skill. Pi discovers
# it through the `pi.skills` manifest entry; omp's native `omp-plugins` provider
# discovers the shipped `skills/` sibling. No extension loader registers it.
# Loading the built dist/index.js file directly is extension-only and must NOT
# expose the skill.
#
# No model and no Bash execution are involved — the fixture replaces the
# model, exactly like scripts/e2e-pi-local.sh.
#
# Usage:
#   AGENT_TERMINAL_AGENT=pi bash scripts/e2e-skill-discovery.sh
#   AGENT_TERMINAL_AGENT=omp bash scripts/e2e-skill-discovery.sh
#
# Environment overrides:
#   AGENT_TERMINAL_AGENT           - agent under test: pi (default) or omp
#   AGENT_TERMINAL_PI_DIR          - pi package dir (default: ~/.local/pi/pkg)
#   AGENT_TERMINAL_FIXTURE_PORT    - port for the fixture server (default: auto — the fixture
#                                  binds an ephemeral port and reports it; no free-port probing)
#   AGENT_TERMINAL_CLEANUP         - delete temp dirs on exit (default: 1)
#   AGENT_TERMINAL_SKIP_PREFLIGHT  - skip build preflight (default: 0)
#   AGENT_TERMINAL_PROMPT_E2E_TIMEOUT - agent run timeout (default: 300)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly PI_ROOT="$REPO_ROOT/pi"
readonly FIXTURE_SCRIPT="$PI_ROOT/scripts/e2e-fixture.ts"

AGENT_TERMINAL_AGENT="${AGENT_TERMINAL_AGENT:-pi}"
if [[ $AGENT_TERMINAL_AGENT == omp ]]; then
  readonly WS_ROOT="$REPO_ROOT/omp"
  readonly BUNDLE_PACKAGE="$WS_ROOT/npm/packages/omp-agent-terminal-bundle-zellij"
else
  readonly WS_ROOT="$PI_ROOT"
  readonly BUNDLE_PACKAGE="$PI_ROOT/npm/packages/pi-agent-terminal-bundle-zellij"
fi

CLEANUP="${AGENT_TERMINAL_CLEANUP:-1}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"
PROMPT_E2E_TIMEOUT="${AGENT_TERMINAL_PROMPT_E2E_TIMEOUT:-300}"

RUN_DIR=""
FIXTURE_PID=""
AGENT_PID=""

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

cleanup() {
  local exit_status=$?
  trap - EXIT INT TERM HUP
  set +e

  if [[ -n $AGENT_PID ]]; then
    kill "$AGENT_PID" 2>/dev/null || true
    wait "$AGENT_PID" 2>/dev/null || true
  fi
  if [[ -n $FIXTURE_PID ]]; then
    kill "$FIXTURE_PID" 2>/dev/null || true
    wait "$FIXTURE_PID" 2>/dev/null || true
  fi

  if [[ $CLEANUP == 1 && -n ${RUN_DIR:-} && -d $RUN_DIR ]]; then
    rm -rf "$RUN_DIR"
  elif [[ -n ${RUN_DIR:-} && -d $RUN_DIR ]]; then
    printf 'Evidence retained at %s\n' "$RUN_DIR"
  fi

  exit "$exit_status"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

# Preflight: required host tools. zellij is NOT checked here: it is provided
# by the bundled package (bin/zellij) and is not exercised by this gate
# (skills are discovered at session start; no tool runs).
missing=()
for tool in bun curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "missing required tools: ${missing[*]}"
fi

# Resolve the agent binary (pi package or omp binary) and build a sandbox
# wrapper for it. pi is not a standalone binary: it runs as
# `bun <package>/dist/cli.js`, so the wrapper execs bun with the package entry.
# omp is a standalone binary and the wrapper just execs it.
case "$AGENT_TERMINAL_AGENT" in
  pi)
    PI_DIR="${AGENT_TERMINAL_PI_DIR:-$HOME/.local/pi/pkg}"
    if [[ ! -f $PI_DIR/dist/cli.js ]]; then
      fail "pi package not found at $PI_DIR (set AGENT_TERMINAL_PI_DIR)"
    fi
    ;;
  omp)
    OMP_BIN="$(command -v omp 2>/dev/null || true)"
    if [[ -z $OMP_BIN && -x /usr/local/bin/omp ]]; then
      OMP_BIN=/usr/local/bin/omp
    fi
    if [[ -z $OMP_BIN ]]; then
      fail "omp binary not found on PATH or at /usr/local/bin/omp"
    fi
    ;;
  *)
    fail "AGENT_TERMINAL_AGENT must be pi or omp, got: $AGENT_TERMINAL_AGENT"
    ;;
esac

# Build the local bundle-zellij npm package (Rust musl binary + extension +
# skill + pinned Zellij) for the agent under test.
if [[ $SKIP_PREFLIGHT != 1 ]]; then
  printf 'Building local %s npm package ...\n' "$AGENT_TERMINAL_AGENT"
  cd "$WS_ROOT"
  if ! bun run build; then
    fail "bun run build failed"
  fi
fi

EXTENSION_PATH="$BUNDLE_PACKAGE/dist/index.js"
if [[ ! -f $EXTENSION_PATH ]]; then
  fail "extension dist missing after build: $EXTENSION_PATH"
fi

# The skill discovery contract requires the bundle package ROOT for both agents:
# pi reads its declared `pi.skills` directory, while omp's native `omp-plugins`
# provider discovers the shipped `skills/` sibling. The built dist/index.js file
# is extension-only and does not expose the skill; it is checked above purely as
# a build-artifact assertion.
EXTENSION_ARG="$BUNDLE_PACKAGE"

# Prepare isolated run directory: sandbox bin dir for the agent wrapper, the
# provider extension file, and the transcript.
RUN_DIR="$(mktemp -d /tmp/e2e-skill-discovery.XXXXXX)"
readonly RUN_DIR
mkdir -p "$RUN_DIR/bin"

AGENT_BIN="$RUN_DIR/bin/$AGENT_TERMINAL_AGENT"
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

# Allocate the fixture port atomically: with --port 0 the kernel binds an
# ephemeral port and the fixture reports the actual port on stdout as
# FIXTURE_PORT=<digits> (no free-port probing, no TOCTOU window).
FIXTURE_PORT="${AGENT_TERMINAL_FIXTURE_PORT:-}"
FIXTURE_ARGS=()
if [[ -z $FIXTURE_PORT ]]; then
  # Auto mode: bind an ephemeral port and parse it back from the log.
  FIXTURE_ARGS+=(--port 0)
else
  FIXTURE_ARGS+=(--port "$FIXTURE_PORT")
fi

# The fixture reads these at process start: AGENT_TERMINAL_AGENT selects the
# probe steps, AGENT_TERMINAL_FIXTURE_SKILL_PROBE=1 switches the fixture into
# skill-probe mode (it asserts the skill description appears in the FIRST
# provider request and replies SKILL_PROBE_OK / SKILL_PROBE_FAILED).
export AGENT_TERMINAL_AGENT
export AGENT_TERMINAL_JOB_NAME="${AGENT_TERMINAL_JOB_NAME:-}"
export AGENT_TERMINAL_FIXTURE_SKILL_PROBE=1

if [[ -z $FIXTURE_PORT ]]; then
  printf 'Starting skill-probe fixture on an ephemeral port ...\n'
else
  printf 'Starting skill-probe fixture on port %s ...\n' "$FIXTURE_PORT"
fi
bun run "$FIXTURE_SCRIPT" "${FIXTURE_ARGS[@]}" >"$RUN_DIR/fixture.log" 2>&1 &
FIXTURE_PID=$!

if [[ -z $FIXTURE_PORT ]]; then
  # Auto mode: the fixture bound an ephemeral port; read it back from the
  # machine-readable FIXTURE_PORT line it prints to stdout.
  for _ in $(seq 1 30); do
    if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
      cat "$RUN_DIR/fixture.log" >&2
      fail "fixture exited before reporting its port (see $RUN_DIR/fixture.log)"
    fi
    FIXTURE_PORT="$(grep -m1 -o 'FIXTURE_PORT=[0-9][0-9]*' "$RUN_DIR/fixture.log" 2>/dev/null | cut -d= -f2 || true)"
    if [[ -n $FIXTURE_PORT ]]; then
      break
    fi
    sleep 0.2
  done
  if [[ -z $FIXTURE_PORT ]]; then
    cat "$RUN_DIR/fixture.log" >&2
    fail "fixture never reported its port (FIXTURE_PORT=<digits> missing from $RUN_DIR/fixture.log)"
  fi
fi

# Wait for the fixture to become healthy.
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$FIXTURE_PORT/health" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
    cat "$RUN_DIR/fixture.log" >&2
    fail "fixture exited before becoming healthy (see $RUN_DIR/fixture.log)"
  fi
  sleep 0.2
done
if ! curl -fsS "http://127.0.0.1:$FIXTURE_PORT/health" >/dev/null 2>&1; then
  fail "fixture did not become healthy within 30 checks"
fi

# The fixture provider is registered by a small extension file that the agent
# loads with -e alongside the agent-terminal extension.
PROVIDER_EXTENSION="$RUN_DIR/provider.ts"
cat >"$PROVIDER_EXTENSION" <<EOF
export default (api: any) => {
  api.registerProvider("local-fixture", {
    baseUrl: "http://127.0.0.1:$FIXTURE_PORT/v1",
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
chmod 0600 "$PROVIDER_EXTENSION"

# Trivial prompt: the agent must not call any tool; skills are still loaded
# and their descriptions must reach the first provider request.
PROMPT_FILE="$RUN_DIR/prompt.md"
cat >"$PROMPT_FILE" <<'PROMPT'
Reply with exactly: OK

Do not use any tools.
PROMPT

TRANSCRIPT="$RUN_DIR/transcript.jsonl"

# Isolated agent environment: the agent runs under `env -i` with a temporary
# HOME, XDG dirs, and agent config dir inside this run's sandbox. Both hosts
# discover user-installed extensions/skills, so a pre-existing install must
# never satisfy the smoke: the package under test and the fixture provider
# are the ONLY things the fresh agent config dir contains. PI_OFFLINE=1 keeps
# pi off the network (the fixture replaces the model).
install -d -m 0700 \
  "$RUN_DIR/home" \
  "$RUN_DIR/config" \
  "$RUN_DIR/data" \
  "$RUN_DIR/cache" \
  "$RUN_DIR/state"

# The agent wrapper resolves bun through PATH, so the isolated PATH carries
# /opt/bun/bin first (the host bun location), then the host PATH.
ISOLATED_PATH="/opt/bun/bin:$PATH"

# Run the agent with skills ENABLED (no --no-skills) and both extensions: the
# agent-terminal bundle package ROOT (static skill discovery) and the fixture
# provider extension. AGENT_FLAGS is intentionally word-split so omp can add
# its native flags.
AGENT_FLAGS=""
if [[ $AGENT_TERMINAL_AGENT == omp ]]; then
  AGENT_FLAGS="--auto-approve --no-pty"
fi

printf 'Running %s agent with skills enabled ...\n' "$AGENT_TERMINAL_AGENT"
# Launch the host from RUN_DIR (a subshell that cds first, the same pattern the
# two-session harness uses): pi's --cwd flag is compatibility-only and does not
# chdir, so without the cd the host would start with its cwd at WS_ROOT and a
# pre-existing project resource could satisfy the smoke. All paths below are
# absolute (derived from REPO_ROOT before any cd), so the cd cannot affect them.
(
  cd "$RUN_DIR"
  # AGENT_FLAGS is intentionally word-split so omp can add its native flags.
  # shellcheck disable=SC2086
  exec env -i \
    HOME="$RUN_DIR/home" \
    USER="$(id -un)" \
    LOGNAME="$(id -un)" \
    SHELL=/bin/bash \
    PATH="$ISOLATED_PATH" \
    LANG=C.UTF-8 \
    TERM=xterm-256color \
    XDG_CONFIG_HOME="$RUN_DIR/config" \
    XDG_DATA_HOME="$RUN_DIR/data" \
    XDG_CACHE_HOME="$RUN_DIR/cache" \
    XDG_STATE_HOME="$RUN_DIR/state" \
    PI_CODING_AGENT_DIR="$RUN_DIR/config" \
    PI_OFFLINE=1 \
    timeout "$PROMPT_E2E_TIMEOUT" "$AGENT_BIN" \
    -p --mode json --model local-fixture/fixture \
    -e "$EXTENSION_ARG" -e "$PROVIDER_EXTENSION" \
    --no-context-files --no-lsp --no-session --thinking off \
    --cwd "$RUN_DIR" $AGENT_FLAGS "$(cat "$PROMPT_FILE")"
) >"$TRANSCRIPT" 2>"$RUN_DIR/stderr.log" &
AGENT_PID=$!

# Wait for the run to finish.
set +e
wait "$AGENT_PID"
AGENT_STATUS=$?
set -e
AGENT_PID=""
if [[ $AGENT_STATUS != 0 ]]; then
  cat "$RUN_DIR/stderr.log" >&2
  fail "agent exited with status $AGENT_STATUS (see $RUN_DIR/stderr.log)"
fi

# The gate: the fixture must have observed the skill description in the first
# provider request and replied SKILL_PROBE_OK.
if grep -q 'SKILL_PROBE_FAILED' "$TRANSCRIPT"; then
  fail "skill probe FAILED: the agent-terminal skill description did not reach the first provider request (see $TRANSCRIPT)"
fi
if ! grep -q 'SKILL_PROBE_OK' "$TRANSCRIPT"; then
  fail "skill probe never completed: transcript lacks SKILL_PROBE_OK (see $TRANSCRIPT)"
fi

printf 'Skill discovery e2e passed (%s).\n' "$AGENT_TERMINAL_AGENT"
