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

AGENT_TERMINAL_TEST_USER="${AGENT_TERMINAL_TEST_USER:-tester-e2e}"
AGENT_TERMINAL_BIN="${AGENT_TERMINAL_BIN:-}"
AGENT_TERMINAL_ENABLE_PROMPT_E2E="${AGENT_TERMINAL_ENABLE_PROMPT_E2E:-1}"
OPENCODE_MODEL="${OPENCODE_MODEL:-litellm/ollama-cloud/deepseek-v4-flash}"
OPENCODE_RUN_FLAGS="${OPENCODE_RUN_FLAGS:-}"
AGENT_TERMINAL_CLEANUP="${AGENT_TERMINAL_CLEANUP:-0}"
AGENT_TERMINAL_OPENCODE_CONFIG="${AGENT_TERMINAL_OPENCODE_CONFIG:-}"
AGENT_TERMINAL_LITELLM_BASE_URL="${AGENT_TERMINAL_LITELLM_BASE_URL:-http://host.docker.internal:57002/v1}"
AGENT_TERMINAL_LITELLM_API_KEY="${AGENT_TERMINAL_LITELLM_API_KEY:-local-no-secret}"
AGENT_TERMINAL_SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"

readonly RUN_ID="$$"
readonly SHORT_BASE="/tmp/ate2e-$$"
readonly WORKDIR="/tmp/agent-terminal-e2e-$AGENT_TERMINAL_TEST_USER-$RUN_ID/workdir"
readonly SANDBOX="/tmp/agent-terminal-e2e-$AGENT_TERMINAL_TEST_USER-$RUN_ID"
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
    rm -rf "$WORKDIR/artifacts"
    cp -a "$ARTIFACT_DIR" "$WORKDIR/artifacts"
  fi

  if [[ $AGENT_TERMINAL_CLEANUP == 1 ]]; then
    rm -rf "$SANDBOX" "$SHORT_BASE"
  else
    printf 'Evidence retained at %s\n' "$SANDBOX"
    rm -rf "$SHORT_BASE"
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
if [[ $AGENT_TERMINAL_ENABLE_PROMPT_E2E == 1 && -z ${AGENT_TERMINAL_OPENCODE_CONFIG:-} ]]; then
  if ! command -v opencode >/dev/null 2>&1; then
    missing+=("opencode")
  fi
fi
if [[ ${#missing[@]} -gt 0 ]]; then
  printf 'error: missing required tools: %s\n' "${missing[*]}" >&2
  exit 1
fi

if [[ $AGENT_TERMINAL_ENABLE_PROMPT_E2E == 1 && $AGENT_TERMINAL_SKIP_PREFLIGHT != 1 ]]; then
  phase "proxy preflight"
  printf 'Preflight: model=%s endpoint=%s\n' "$OPENCODE_MODEL" "$AGENT_TERMINAL_LITELLM_BASE_URL"
  if ! curl -fsS "$AGENT_TERMINAL_LITELLM_BASE_URL/models" -H "Authorization: Bearer $AGENT_TERMINAL_LITELLM_API_KEY" >/dev/null 2>"$SANDBOX/preflight.log"; then
    printf 'error: cannot reach LLM endpoint %s/models (see %s/preflight.log)\n' "$AGENT_TERMINAL_LITELLM_BASE_URL" "$SANDBOX" >&2
    exit 1
  fi
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
install -m 0644 "$SKILL_SOURCE" "$CONFIG_DIR/skills/agent-terminal/SKILL.md"

if [[ -n $AGENT_TERMINAL_OPENCODE_CONFIG ]]; then
  if [[ ! -f $AGENT_TERMINAL_OPENCODE_CONFIG ]]; then
    printf 'error: AGENT_TERMINAL_OPENCODE_CONFIG points to a missing file: %s\n' "$AGENT_TERMINAL_OPENCODE_CONFIG" >&2
    exit 1
  fi
  install -m 0600 "$AGENT_TERMINAL_OPENCODE_CONFIG" "$CONFIG_DIR/opencode.json"
else
  if [[ $OPENCODE_MODEL != litellm/* ]]; then
    printf 'error: default generated opencode.json only supports litellm/* models. Set AGENT_TERMINAL_OPENCODE_CONFIG for other providers.\n' >&2
    exit 1
  fi
  model_alias="${OPENCODE_MODEL#litellm/}"
  python3 - "$AGENT_TERMINAL_LITELLM_BASE_URL" "$AGENT_TERMINAL_LITELLM_API_KEY" "$model_alias" "$OPENCODE_MODEL" <<'PY' >"$CONFIG_DIR/opencode.json"
import json, sys
base_url, api_key, alias, full_name = sys.argv[1:5]
config = {
    "$schema": "https://opencode.ai/config.json",
    "autoupdate": False,
    "share": "disabled",
    "permission": {"*": "allow", "skill": {"*": "allow"}},
    "disabled_providers": ["opencode"],
    "plugin": [],
    "provider": {
        "litellm": {
            "npm": "@ai-sdk/openai-compatible",
            "name": "LiteLLM (local)",
            "options": {
                "baseURL": base_url,
                "apiKey": api_key,
                "autoload": False,
            },
            "models": {
                alias: {
                    "name": full_name,
                    "tool_call": True,
                    "limit": {"context": 500000, "output": 65536},
                }
            },
        }
    },
}
json.dump(config, sys.stdout, indent=2)
PY
  chmod 0600 "$CONFIG_DIR/opencode.json"
fi

readonly PROMPT_FILE="$WORKDIR/prompt.md"
cat >"$PROMPT_FILE" <<EOF
This is an automated end-to-end test of the installed agent-terminal skill. Work only in $WORKDIR. Use Bash to execute each command, inspect every JSON response, and do not skip or reorder steps.

Perform this exact 9-step lifecycle using the job name prompt-smoke-$RUN_ID:
1. Run agent-terminal list and verify the JSON response has status ok and an empty jobs array.
2. Start the interactive job with: agent-terminal start prompt-smoke-$RUN_ID -- /bin/bash -lc 'printf "prompt-ready\\n"; IFS= read -r first; printf "first:%s\\n" "\$first"; IFS= read -r second; printf "second:%s\\n" "\$second"'
3. Read prompt-smoke-$RUN_ID and verify its JSON screen contains prompt-ready. Retry read briefly only if the pane has not rendered it yet.
4. Send the literal text hello-e2e with: agent-terminal send prompt-smoke-$RUN_ID -- hello-e2e
5. Read prompt-smoke-$RUN_ID and verify its JSON screen contains first:hello-e2e.
6. Press Enter with: agent-terminal press prompt-smoke-$RUN_ID -- Enter
7. Read prompt-smoke-$RUN_ID and verify the JSON reports state exited with exit_code 0 and its screen contains second:.
8. Stop prompt-smoke-$RUN_ID and verify the JSON reports cleaned_up true.
9. Run agent-terminal list and verify the JSON response has status ok and an empty jobs array.

Do not modify repository files. After all nine steps pass, end your final response with a separate line containing exactly E2E_SUCCESS. Do not print E2E_SUCCESS if any step fails.
EOF

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
    if (body.status !== "ok" || typeof body.data !== "object" || body.data === null) {
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
  if (!Array.isArray(body.data.jobs) || body.data.jobs.length !== 0) {
    throw new Error(`initial list was not empty: ${JSON.stringify(body)}`)
  }
'

run_cli start start smoke -- /bin/sh -c 'printf "smoke-ready\n"; trap "exit 0" INT; while :; do sleep 1; done'
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (body.data.job !== "smoke" || body.data.state !== "running") {
    throw new Error(`smoke job did not start: ${JSON.stringify(body)}`)
  }
'

run_cli read read smoke
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (body.data.job !== "smoke" || !["running", "exited"].includes(body.data.state)) {
    throw new Error(`smoke job could not be read: ${JSON.stringify(body)}`)
  }
'

run_cli stop stop smoke
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (body.data.job !== "smoke" || body.data.cleaned_up !== true) {
    throw new Error(`smoke job was not cleaned up: ${JSON.stringify(body)}`)
  }
'

run_cli list-after list
JSON_FILE="$LAST_JSON" bun -e '
  const body = await Bun.file(process.env.JSON_FILE).json()
  if (!Array.isArray(body.data.jobs) || body.data.jobs.length !== 0) {
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
  # OPENCODE_RUN_FLAGS is intentionally word-split so callers can supply multiple CLI flags.
  # shellcheck disable=SC2086
  if ! opencode --pure run --auto --format json --dir "$WORKDIR" --model "$OPENCODE_MODEL" \
    $OPENCODE_RUN_FLAGS -- "$(cat "$PROMPT_FILE")" \
    >"$ARTIFACT_DIR/prompt-e2e.jsonl" 2>"$ARTIFACT_DIR/prompt-e2e.stderr.log"; then
    cat "$ARTIFACT_DIR/prompt-e2e.stderr.log" >&2
    cat "$ARTIFACT_DIR/prompt-e2e.jsonl" >&2
    printf 'error: opencode prompt e2e exited nonzero\n' >&2
    exit 1
  fi

  cat "$ARTIFACT_DIR/prompt-e2e.stderr.log" >&2
  cat "$ARTIFACT_DIR/prompt-e2e.jsonl"
  PROMPT_LOG="$ARTIFACT_DIR/prompt-e2e.jsonl" bun -e '
    const text = await Bun.file(process.env.PROMPT_LOG).text()
    if (!text.includes("E2E_SUCCESS")) {
      throw new Error("OpenCode output did not contain E2E_SUCCESS")
    }
    const lines = text.split(/\r?\n/).filter((line) => line.trim() !== "")
    if (lines.length === 0) throw new Error("OpenCode produced no JSON events")
    for (const line of lines) {
      const event = JSON.parse(line)
      if (event.type === "error") {
        throw new Error(`OpenCode emitted an error event: ${line}`)
      }
      const state = event.part?.state ?? event.state
      if (event.type === "tool_use" && state?.status === "error") {
        throw new Error(`OpenCode emitted an error tool event: ${line}`)
      }
    }
  '
fi

printf '\nAll executed e2e phases passed.\n'
INNER

chmod 0755 "$INNER_SCRIPT"


phase "e2e execution"
printf 'Running e2e phases as %s; artifacts: %s\n' "$(id -un)" "$ARTIFACT_DIR"
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
  OPENCODE_DISABLE_MODELS_FETCH=1 \
  OPENCODE_DISABLE_LSP_DOWNLOAD=1 \
  ZELLIJ_SOCKET_DIR="$ZELLIJ_SOCKET_DIR" \
  AGENT_TERMINAL_STATE="$STATE_DIR" \
  AGENT_TERMINAL_ENABLE_PROMPT_E2E="$AGENT_TERMINAL_ENABLE_PROMPT_E2E" \
  OPENCODE_MODEL="$OPENCODE_MODEL" \
  OPENCODE_RUN_FLAGS="$OPENCODE_RUN_FLAGS" \
  WORKDIR="$WORKDIR" \
  ARTIFACT_DIR="$ARTIFACT_DIR" \
  PROMPT_FILE="$PROMPT_FILE" \
  script -q -e -E never -c "$INNER_SCRIPT" "$ARTIFACT_DIR/transcript.log"
