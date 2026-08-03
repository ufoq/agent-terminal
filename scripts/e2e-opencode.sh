#!/usr/bin/env bash

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly SKILL_SOURCE="$REPO_ROOT/opencode/skills/agent-terminal/SKILL.md"

repo_owner=""
if command -v stat >/dev/null 2>&1; then
  repo_owner="$(stat -c '%U' "$REPO_ROOT" 2>/dev/null || true)"
fi

ORIG_PATH="$PATH"
if [[ -z ${ORIG_PATH:-} ]]; then
  printf 'error: PATH is empty or unset\n' >&2
  exit 1
fi
HOST_PATH="$ORIG_PATH"

AGENT_TERMINAL_RUN_PREFIX="${AGENT_TERMINAL_RUN_PREFIX:-e2e}"
AGENT_TERMINAL_BIN="${AGENT_TERMINAL_BIN:-}"
AGENT_TERMINAL_ENABLE_PROMPT_E2E="${AGENT_TERMINAL_ENABLE_PROMPT_E2E:-1}"
OPENCODE_MODEL="${OPENCODE_MODEL:-}"
OPENCODE_RUN_FLAGS="${OPENCODE_RUN_FLAGS:-}"
AGENT_TERMINAL_CLEANUP="${AGENT_TERMINAL_CLEANUP:-0}"
AGENT_TERMINAL_OPENCODE_CONFIG="${AGENT_TERMINAL_OPENCODE_CONFIG:-}"
AGENT_TERMINAL_OPENCODE_AUTH="${AGENT_TERMINAL_OPENCODE_AUTH:-}"
AGENT_TERMINAL_VERIFY_MODE="${AGENT_TERMINAL_VERIFY_MODE:-strict}"
AGENT_TERMINAL_DISABLE_MODELS_FETCH="${AGENT_TERMINAL_DISABLE_MODELS_FETCH:-1}"
AGENT_TERMINAL_SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"
AGENT_TERMINAL_HOST_PATH="${AGENT_TERMINAL_HOST_PATH:-$HOST_PATH}"

readonly RUN_ID="$$"
readonly JOB_NAME="prompt-smoke-$RUN_ID"
readonly SHORT_BASE="/tmp/ate2e-$$"
readonly WORKDIR="/tmp/agent-terminal-$AGENT_TERMINAL_RUN_PREFIX-$RUN_ID/workdir"
readonly SANDBOX="/tmp/agent-terminal-$AGENT_TERMINAL_RUN_PREFIX-$RUN_ID"
readonly CONFIG_DIR="$SANDBOX/config"
readonly DATA_DIR="$SANDBOX/data"
readonly CACHE_DIR="$SANDBOX/cache"
readonly STATE_DIR="$SANDBOX/state"
readonly ZELLIJ_SOCKET_DIR="$SHORT_BASE/z"
readonly ARTIFACT_DIR="$SANDBOX/artifacts"

if [[ -e $WORKDIR || -e $SANDBOX ]]; then
  printf 'error: per-run path already exists for PID %s\n' "$RUN_ID" >&2
  exit 1
fi
install -d -m 0700 "$SANDBOX"
cleanup() {
  local exit_status=$?
  local sessions=""
  local session=""

  trap - EXIT INT TERM HUP
  set +e

  if [[ -d $ZELLIJ_SOCKET_DIR ]]; then
    sessions="$(env ZELLIJ_SOCKET_DIR="$ZELLIJ_SOCKET_DIR" zellij list-sessions --short --no-formatting 2>/dev/null || true)"
    while IFS= read -r session; do
      if [[ $session == agent-terminal-* ]]; then
        env ZELLIJ_SOCKET_DIR="$ZELLIJ_SOCKET_DIR" zellij kill-session "$session" >/dev/null 2>&1 || true
      fi
    done <<<"$sessions"
  fi

  if [[ $AGENT_TERMINAL_CLEANUP != 1 && -d $ARTIFACT_DIR ]]; then
    EVIDENCE_DIR="/tmp/agent-terminal-$AGENT_TERMINAL_RUN_PREFIX-$RUN_ID-evidence"
    rm -rf "$EVIDENCE_DIR"
    install -d -m 0700 "$EVIDENCE_DIR"
    cp -a "$ARTIFACT_DIR/." "$EVIDENCE_DIR/"
  fi

  # Config, auth, cache and state may contain projected provider credentials and
  # are NEVER retained; only the artifact directory is kept as evidence.
  rm -rf "$SANDBOX" "$SHORT_BASE"
  if [[ -n ${EVIDENCE_DIR:-} && -d $EVIDENCE_DIR ]]; then
    printf 'Evidence retained at %s\n' "$EVIDENCE_DIR"
  fi

  exit "$exit_status"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

PHASE="init"
phase() { PHASE="$1"; }
fail_phase() {
  printf 'error: phase "%s" failed\n' "$PHASE" >&2
  exit 1
}
trap fail_phase ERR

missing=()
for tool in bun zellij script; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ -n ${AGENT_TERMINAL_BIN:-} ]]; then
  if [[ ! -x $AGENT_TERMINAL_BIN ]]; then
    printf 'error: AGENT_TERMINAL_BIN is set to a non-executable file: %s\n' "$AGENT_TERMINAL_BIN" >&2
    exit 1
  fi
else
  if ! command -v cargo >/dev/null 2>&1; then
    missing+=("cargo")
  fi
fi
if [[ $AGENT_TERMINAL_ENABLE_PROMPT_E2E == 1 ]]; then
  if ! command -v opencode >/dev/null 2>&1; then
    missing+=("opencode")
  fi
  if [[ -z ${AGENT_TERMINAL_OPENCODE_CONFIG:-} ]]; then
    missing+=("AGENT_TERMINAL_OPENCODE_CONFIG")
  fi
  if [[ -z ${OPENCODE_MODEL:-} ]]; then
    missing+=("OPENCODE_MODEL")
  fi
fi
if [[ ${#missing[@]} -gt 0 ]]; then
  printf 'error: missing required tools: %s\n' "${missing[*]}" >&2
  exit 1
fi

phase "binary setup"
if [[ -n ${AGENT_TERMINAL_BIN:-} ]]; then
  resolved_binary="$AGENT_TERMINAL_BIN"
else
  resolved_binary="$REPO_ROOT/target/release/agent-terminal"
  if ! command -v cargo >/dev/null 2>&1 && [[ -f $HOME/.cargo/env ]]; then
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1 && [[ -n ${repo_owner:-} && -f /home/$repo_owner/.cargo/env ]]; then
    # shellcheck source=/dev/null
    . "/home/$repo_owner/.cargo/env"
  fi
  if ! command -v cargo >/dev/null 2>&1; then
    printf 'error: cargo not found on PATH\n' >&2
    exit 1
  fi
  setup_tmp="$(mktemp -d /tmp/agent-terminal-e2e-build-XXXXXX)"
  if ! cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" >"$setup_tmp/setup.log" 2>&1; then
    printf 'error: cargo build --release failed\n' >&2
    cat "$setup_tmp/setup.log" >&2
    rm -rf "$setup_tmp"
    exit 1
  fi
  rm -rf "$setup_tmp"
fi

if [[ ! -f $resolved_binary || ! -x $resolved_binary ]]; then
  printf 'error: agent-terminal binary is not an executable file: %s\n' "$resolved_binary" >&2
  exit 1
fi

install -d -m 0700 \
  "$WORKDIR" \
  "$CONFIG_DIR" \
  "$CONFIG_DIR/skills" \
  "$CONFIG_DIR/skills/agent-terminal" \
  "$DATA_DIR" \
  "$CACHE_DIR" \
  "$STATE_DIR" \
  "$ZELLIJ_SOCKET_DIR" \
  "$ARTIFACT_DIR"

if [[ $AGENT_TERMINAL_ENABLE_PROMPT_E2E == 1 ]]; then
  if [[ -n $AGENT_TERMINAL_OPENCODE_CONFIG ]]; then
    if [[ ! -f $AGENT_TERMINAL_OPENCODE_CONFIG ]]; then
      printf 'error: AGENT_TERMINAL_OPENCODE_CONFIG points to a missing file: %s\n' "$AGENT_TERMINAL_OPENCODE_CONFIG" >&2
      exit 1
    fi
    install -m 0600 "$AGENT_TERMINAL_OPENCODE_CONFIG" "$CONFIG_DIR/opencode.json"
    if [[ -n $AGENT_TERMINAL_OPENCODE_AUTH ]]; then
      if [[ ! -f $AGENT_TERMINAL_OPENCODE_AUTH ]]; then
        printf 'error: AGENT_TERMINAL_OPENCODE_AUTH points to a missing file: %s\n' "$AGENT_TERMINAL_OPENCODE_AUTH" >&2
        exit 1
      fi
      install -d -m 0700 "$DATA_DIR/opencode"
      install -m 0600 "$AGENT_TERMINAL_OPENCODE_AUTH" "$DATA_DIR/opencode/auth.json"
    fi
  else
    printf 'error: AGENT_TERMINAL_OPENCODE_CONFIG must point to an OpenCode config when prompt e2e is enabled\n' >&2
    exit 1
  fi
fi

readonly PROMPT_FILE="$WORKDIR/prompt.md"
sed -e "s/__RUN_ID__/$RUN_ID/g" -e "s|__WORKDIR__|$WORKDIR|g" >"$PROMPT_FILE" <<'PROMPT_EOF'
This is an execution task, not a writing or planning task. Work only in __WORKDIR__. Do not output any prose, Markdown, YAML, code block, template, plan, or simulated transcript.

Execution contract:
- The ONLY tool you may use is Bash. Every assistant turn must emit exactly one Bash tool call for the first incomplete numbered step below.
- Wait for the real Bash command's JSON output, inspect it, and then emit the next Bash tool call. Do not predict or invent command output.
- You must NOT call the read, write, task, or any other non-Bash tool for any purpose. The word "read" below always means the Bash command `agent-terminal read`, never the read tool.
- You must NOT delegate to a subagent, create files, or emit simulated/agent-terminal commands inside Markdown or YAML blocks.
- If any step fails, stop and report the failure; only end with E2E_SUCCESS after step 10 passes.

Perform this exact 10-step lifecycle using the job name prompt-smoke-__RUN_ID__:
0. Bash: run `printenv AGENT_TERMINAL_SCOPE` and confirm it prints the current OpenCode session id (non-empty). This verifies the plugin injected the per-session scope.
1. Bash: `agent-terminal list` and verify the JSON response has status ok and an empty jobs array.
2. Bash: start the interactive job with `agent-terminal start prompt-smoke-__RUN_ID__ -- /bin/bash -lc 'printf "prompt-ready\n"; IFS= read -r first; printf "first:%s\n" "$first"; IFS= read -r second; printf "second:%s\n" "$second"'`.
3. Bash: `agent-terminal read prompt-smoke-__RUN_ID__` and verify its JSON screen contains prompt-ready. Retry the Bash read command briefly only if the pane has not rendered it yet.
4. Bash: send the literal text hello-e2e with `agent-terminal send prompt-smoke-__RUN_ID__ -- hello-e2e`.
5. Bash: `agent-terminal read prompt-smoke-__RUN_ID__` and verify its JSON screen contains first:hello-e2e.
6. Bash: press Enter with `agent-terminal press prompt-smoke-__RUN_ID__ -- Enter`.
7. Bash: `agent-terminal read prompt-smoke-__RUN_ID__` and verify the JSON reports state exited with exit_code 0 and its screen contains second:.
8. Bash: `agent-terminal stop prompt-smoke-__RUN_ID__` and verify the JSON response is the status ok acknowledgement.
9. Bash: `agent-terminal list` and verify the JSON response has status ok and an empty jobs array.

Do not modify repository files. After all ten steps pass, end your final response with a separate line containing exactly E2E_SUCCESS. Do not print E2E_SUCCESS if any step fails.
PROMPT_EOF

readonly INNER_SCRIPT="$SANDBOX/run-e2e-inner.sh"
cat >"$INNER_SCRIPT" <<'INNER'
#!/usr/bin/env bash

set -Eeuo pipefail

PHASE="init"
phase() { PHASE="$1"; }
fail_phase() {
  printf 'error: phase "%s" failed\n' "$PHASE" >&2
  exit 1
}
trap fail_phase ERR

: >"$ARTIFACT_DIR/bun-test.log"
: >"$ARTIFACT_DIR/cli-smoke.log"
: >"$ARTIFACT_DIR/prompt-e2e.jsonl"
: >"$ARTIFACT_DIR/prompt-e2e.stderr.log"

phase "OpenCode skill tests"
printf '== OpenCode skill tests ==\n'
cd "$REPO_ROOT/opencode"
bun test 2>&1 | tee "$ARTIFACT_DIR/bun-test.log"

LAST_JSON=""
run_cli() {
  local label="$1"
  shift
  local stdout_file="$ARTIFACT_DIR/cli-$label.json"
  local stderr_file="$ARTIFACT_DIR/cli-$label.stderr.log"

  printf '\n$ agent-terminal %s\n' "$*" | tee -a "$ARTIFACT_DIR/cli-smoke.log"
  if ! agent-terminal "$@" >"$stdout_file" 2>"$stderr_file"; then
    cat "$stderr_file" | tee -a "$ARTIFACT_DIR/cli-smoke.log" >&2
    cat "$stdout_file" | tee -a "$ARTIFACT_DIR/cli-smoke.log" >&2
    printf 'error: agent-terminal %s failed\n' "$*" >&2
    return 1
  fi
  cat "$stderr_file" | tee -a "$ARTIFACT_DIR/cli-smoke.log" >&2
  cat "$stdout_file" | tee -a "$ARTIFACT_DIR/cli-smoke.log"
  JSON_FILE="$stdout_file" bun -e '
    const body = await Bun.file(process.env.JSON_FILE).json()
    if (typeof body !== "object" || body === null || Array.isArray(body) || body.status !== "ok") {
      throw new Error(`unexpected agent-terminal response: ${JSON.stringify(body)}`)
    }
  '
  LAST_JSON="$stdout_file"
}

phase "direct CLI smoke test"
printf '\n== Direct CLI smoke test ==\n' | tee -a "$ARTIFACT_DIR/cli-smoke.log"
cd "$WORKDIR"

run_cli list-before list
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (!Array.isArray(body.jobs) || body.jobs.length !== 0) {
    throw new Error(`initial list was not empty: ${JSON.stringify(body)}`)
  }
'

run_cli start start smoke -- /bin/sh -c 'printf "smoke-ready\n"; trap "exit 0" INT; while :; do sleep 1; done'
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (body.state !== "running") {
    throw new Error(`smoke job did not start: ${JSON.stringify(body)}`)
  }
'

run_cli read read smoke
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (!["running", "exited"].includes(body.state) || typeof body.screen !== "string") {
    throw new Error(`smoke job could not be read: ${JSON.stringify(body)}`)
  }
'

run_cli stop stop smoke
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (Object.keys(body).length !== 1) {
    throw new Error(`smoke job was not cleaned up: ${JSON.stringify(body)}`)
  }
'

run_cli list-after list
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (!Array.isArray(body.jobs) || body.jobs.length !== 0) {
    throw new Error(`final list was not empty: ${JSON.stringify(body)}`)
  }
'

remaining_sessions="$(zellij list-sessions --short --no-formatting 2>/dev/null || true)"
if [[ $remaining_sessions == *agent-terminal-* ]]; then
  printf 'error: direct CLI smoke test left a Zellij session behind: %s\n' "$remaining_sessions" \
    | tee -a "$ARTIFACT_DIR/cli-smoke.log" >&2
  exit 1
fi

if [[ $AGENT_TERMINAL_ENABLE_PROMPT_E2E == 1 ]]; then
  phase "OpenCode prompt e2e"
  printf '\n== OpenCode prompt e2e ==\n'

  # The packaged plugin's config hook must register the agent-terminal skill.
  # No manual skill install happens in this harness, so this check fails if the
  # plugin fails to expose its bundled skills directory.
  if ! timeout 30 opencode debug skill 2>/dev/null | grep -q '"name": "agent-terminal"'; then
    printf 'error: agent-terminal skill was not registered by the plugin config hook\n' >&2
    exit 1
  fi

  # OPENCODE_RUN_FLAGS is intentionally word-split so callers can supply multiple CLI flags.
  # shellcheck disable=SC2086
  PROMPT_E2E_TIMEOUT="${AGENT_TERMINAL_PROMPT_E2E_TIMEOUT:-600}"
  # Run opencode with a PATH that does not contain the bundled binaries. The
  # plugin's factory-time process.env.PATH mutation still exposes them, so the
  # clean PATH alone does NOT prove the shell.env hook fired. Scope injection
  # (AGENT_TERMINAL_SCOPE) is proven separately by the scope-probe Bash call
  # that the verifier asserts is the first tool_use event and that it matches
  # the transcript's sessionID.
  if ! PATH="$AGENT_TERMINAL_HOST_PATH" timeout "$PROMPT_E2E_TIMEOUT" opencode run --auto --format json --dir "$WORKDIR" --model "$OPENCODE_MODEL" \
    $OPENCODE_RUN_FLAGS -- "$(cat "$PROMPT_FILE")" \
    >"$ARTIFACT_DIR/prompt-e2e.jsonl" 2>"$ARTIFACT_DIR/prompt-e2e.stderr.log"; then
    cat "$ARTIFACT_DIR/prompt-e2e.stderr.log" >&2
    cat "$ARTIFACT_DIR/prompt-e2e.jsonl" >&2
    printf 'error: opencode prompt e2e failed or exceeded %ss timeout\n' "$PROMPT_E2E_TIMEOUT" >&2
    exit 1
  fi

  cat "$ARTIFACT_DIR/prompt-e2e.stderr.log" >&2
  cat "$ARTIFACT_DIR/prompt-e2e.jsonl"
  PROMPT_LOG="$ARTIFACT_DIR/prompt-e2e.jsonl" bun run "$REPO_ROOT/opencode/scripts/e2e-verify.ts" "$ARTIFACT_DIR/prompt-e2e.jsonl" "$JOB_NAME" --mode "$AGENT_TERMINAL_VERIFY_MODE" --workdir "$WORKDIR"
fi

printf '\nAll executed e2e phases passed.\n'
INNER

chmod 0755 "$INNER_SCRIPT"


phase "e2e execution"
printf 'Running e2e phases as %s; artifacts: %s\n' "$(id -un)" "$ARTIFACT_DIR"

# Pass through the explicitly allowlisted provider env vars (values are stripped
# by env -i otherwise). Everything else stays out of the sandbox.
PROVIDER_ENV_PASSTHRU=()
if [[ -n ${AGENT_TERMINAL_PROVIDER_ENV_VARS:-} ]]; then
  IFS=',' read -ra _env_names <<<"$AGENT_TERMINAL_PROVIDER_ENV_VARS"
  for _name in "${_env_names[@]}"; do
    _name="${_name//[[:space:]]/}"
    if [[ -z $_name ]]; then
      continue
    fi
    if [[ ! $_name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      fail "invalid provider environment variable name: $_name"
    fi
    if [[ -n ${!_name:-} ]]; then
      PROVIDER_ENV_PASSTHRU+=("$_name=${!_name}")
    fi
  done
fi

env -i \
  HOME="$(getent passwd "$(id -un)" | cut -d: -f6)" \
  REPO_ROOT="$REPO_ROOT" \
  USER="$(id -un)" \
  LOGNAME="$(id -un)" \
  SHELL=/bin/bash \
  PATH="$HOST_PATH" \
  LANG=C.UTF-8 \
  TERM=xterm-256color \
  HISTFILE=/dev/null \
  BASH_ENV=/dev/null \
  ENV=/dev/null \
  ZDOTDIR="$SANDBOX/home" \
  XDG_CONFIG_HOME="$CONFIG_DIR" \
  XDG_DATA_HOME="$DATA_DIR" \
  XDG_CACHE_HOME="$CACHE_DIR" \
  XDG_STATE_HOME="$STATE_DIR" \
  OPENCODE_CONFIG_DIR="$CONFIG_DIR" \
  OPENCODE_DISABLE_PROJECT_CONFIG=1 \
  OPENCODE_DISABLE_EXTERNAL_SKILLS=1 \
  OPENCODE_DISABLE_CLAUDE_CODE=1 \
  OPENCODE_DISABLE_AUTOUPDATE=1 \
  OPENCODE_DISABLE_MODELS_FETCH="$AGENT_TERMINAL_DISABLE_MODELS_FETCH" \
  OPENCODE_DISABLE_LSP_DOWNLOAD=1 \
  ZELLIJ_SOCKET_DIR="$ZELLIJ_SOCKET_DIR" \
  AGENT_TERMINAL_STATE="$STATE_DIR" \
  AGENT_TERMINAL_BIN="${AGENT_TERMINAL_BIN:-}" \
  AGENT_TERMINAL_ENABLE_PROMPT_E2E="$AGENT_TERMINAL_ENABLE_PROMPT_E2E" \
  AGENT_TERMINAL_HOST_PATH="$AGENT_TERMINAL_HOST_PATH" \
  AGENT_TERMINAL_VERIFY_MODE="$AGENT_TERMINAL_VERIFY_MODE" \
  AGENT_TERMINAL_OPENCODE_AUTH="$AGENT_TERMINAL_OPENCODE_AUTH" \
  AGENT_TERMINAL_PROVIDER_ENV_VARS="${AGENT_TERMINAL_PROVIDER_ENV_VARS:-}" \
  OPENCODE_MODEL="$OPENCODE_MODEL" \
  OPENCODE_RUN_FLAGS="$OPENCODE_RUN_FLAGS" \
  WORKDIR="$WORKDIR" \
  ARTIFACT_DIR="$ARTIFACT_DIR" \
  PROMPT_FILE="$PROMPT_FILE" \
  JOB_NAME="$JOB_NAME" \
  "${PROVIDER_ENV_PASSTHRU[@]}" \
  script -q -e -E never -c "$INNER_SCRIPT" "$ARTIFACT_DIR/transcript.log"
