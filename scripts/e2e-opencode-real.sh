#!/usr/bin/env bash
#
# Optional real-model e2e for agent-terminal (LOCAL ONLY, not a release gate).
#
# Runs the full 9-step lifecycle through OpenCode against a REAL model from the
# user's own OpenCode configuration. Unlike scripts/e2e-opencode-local.sh (the
# deterministic fixture gate), this script does not replace the model - it lets
# a real model discover the packaged skill and drive agent-terminal end to end.
#
# The sandbox never sees the user's full config: only the selected provider
# block is projected, only that provider's auth.json entry is copied, and only
# allowlisted env vars are passed through. The sandbox config/auth are always
# deleted, even on failure.
#
# Usage:
#   AGENT_TERMINAL_OPENCODE_CONFIG=~/.config/opencode/opencode.json \
#   OPENCODE_MODEL=litellm/ollama-cloud/deepseek-v4-flash \
#   bash scripts/e2e-opencode-real.sh
#
# Environment:
#   AGENT_TERMINAL_OPENCODE_CONFIG   path to the user's real opencode.json/JSONC
#                                    (default: ~/.config/opencode/opencode.json)
#   OPENCODE_MODEL                   provider/model to test (required, e.g.
#                                    litellm/ollama-cloud/deepseek-v4-flash)
#   AGENT_TERMINAL_OPENCODE_AUTH     auth.json to project credentials from
#                                    (default: ~/.local/share/opencode/auth.json)
#   AGENT_TERMINAL_PROVIDER_ENV_VARS comma-separated env vars passed through
#                                    (e.g. "OPENAI_API_KEY,ANTHROPIC_API_KEY")
#   AGENT_TERMINAL_PROMPT_E2E_TIMEOUT opencode run timeout in seconds (default 900)
#   AGENT_TERMINAL_ALLOW_REAL_MODEL_CI set to 1 to allow running under CI
#   AGENT_TERMINAL_CLEANUP           delete wrapper worktree on exit (default: 0)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly E2E_HARNESS="$REPO_ROOT/scripts/e2e-opencode.sh"
readonly NPM_ROOT="$REPO_ROOT/opencode/npm"
readonly BUNDLE_PACKAGE="$NPM_ROOT/packages/opencode-agent-terminal-bundle-zellij"

CLEANUP="${AGENT_TERMINAL_CLEANUP:-0}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"
RUN_DIR=""
SANDBOX_CONFIG=""
SANDBOX_AUTH=""

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

cleanup() {
  local exit_status=$?
  trap - EXIT INT TERM HUP
  set +e

  # Credentials never survive: scoped config and auth are always removed.
  if [[ -n $SANDBOX_CONFIG && -f $SANDBOX_CONFIG ]]; then
    rm -f "$SANDBOX_CONFIG"
  fi
  if [[ -n $SANDBOX_AUTH && -f $SANDBOX_AUTH ]]; then
    rm -f "$SANDBOX_AUTH"
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

# Refuse to run in CI unless explicitly allowed: this test consumes real model
# tokens and is meant for interactive local use, not automated pipelines.
if [[ -n ${CI:-} && ${AGENT_TERMINAL_ALLOW_REAL_MODEL_CI:-0} != 1 ]]; then
  fail "refusing to run the real-model e2e under CI (set AGENT_TERMINAL_ALLOW_REAL_MODEL_CI=1 to override)"
fi

USER_CONFIG="${AGENT_TERMINAL_OPENCODE_CONFIG:-$HOME/.config/opencode/opencode.json}"
OPENCODE_MODEL="${OPENCODE_MODEL:-}"
if [[ -z $OPENCODE_MODEL ]]; then
  fail "OPENCODE_MODEL is required (e.g. litellm/ollama-cloud/deepseek-v4-flash)"
fi
if [[ ! -f $USER_CONFIG ]]; then
  fail "config not found: $USER_CONFIG (set AGENT_TERMINAL_OPENCODE_CONFIG)"
fi
# Capture the source before clearing the inherited selector; the wrapper must only
# project this immutable path's selected provider entry.
readonly USER_AUTH="${AGENT_TERMINAL_OPENCODE_AUTH:-$HOME/.local/share/opencode/auth.json}"
unset AGENT_TERMINAL_OPENCODE_AUTH
PROVIDER_ENV_VARS="${AGENT_TERMINAL_PROVIDER_ENV_VARS:-}"

PROVIDER_ID="${OPENCODE_MODEL%%/*}"
if [[ -z $PROVIDER_ID || $PROVIDER_ID == "$OPENCODE_MODEL" ]]; then
  fail "OPENCODE_MODEL must be in provider/model form, got: $OPENCODE_MODEL"
fi
MODEL_ALIAS="${OPENCODE_MODEL#*/}"

missing=()
for tool in bun npm opencode python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "missing required tools: ${missing[*]}"
fi

# Build and pack the local bundle-zellij npm package (same as the fixture gate).
if [[ $SKIP_PREFLIGHT != 1 ]]; then
  printf 'Building local npm plugin package ...\n'
  cd "$REPO_ROOT/opencode"
  if ! bun run build; then
    fail "bun run build failed"
  fi
fi

cd "$BUNDLE_PACKAGE"
TARBALL="$(npm pack --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["filename"])')"
TARBALL_PATH="$BUNDLE_PACKAGE/$TARBALL"
if [[ ! -f $TARBALL_PATH ]]; then
  fail "npm pack did not produce a tarball"
fi

RUN_DIR="$(mktemp -d /tmp/e2e-real-repository.XXXXXX)"
readonly RUN_DIR
mkdir -p "$RUN_DIR/.opencode" "$RUN_DIR/node_modules"

if ! npm install --prefix "$RUN_DIR" "$TARBALL_PATH" --no-save --no-audit --no-fund >/dev/null 2>&1; then
  fail "failed to install packed plugin into $RUN_DIR"
fi

PLUGIN_PATH="$RUN_DIR/node_modules/@ufoq/opencode-agent-terminal-bundle-zellij/dist/index.js"
if [[ ! -f $PLUGIN_PATH ]]; then
  fail "plugin dist missing after npm install: $PLUGIN_PATH"
fi

SANDBOX_CONFIG="$RUN_DIR/.opencode/opencode.json"
SANDBOX_AUTH="$RUN_DIR/.opencode/auth.json"

# Project ONLY the selected provider block (plus the packed plugin and safe
# permissions) into a scoped sandbox config. The user's other providers,
# plugins, MCPs, agents, commands and instructions never reach the sandbox.
python3 - "$USER_CONFIG" "$PROVIDER_ID" "$MODEL_ALIAS" "$OPENCODE_MODEL" "$PLUGIN_PATH" "$SANDBOX_CONFIG" <<'PY'
import json
import sys

def strip_jsonc(text: str) -> str:
    out = []
    i = 0
    n = len(text)
    in_string = False
    while i < n:
        c = text[i]
        if in_string:
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 2
                continue
            if c == '"':
                in_string = False
            i += 1
            continue
        if c == '"':
            in_string = True
            out.append(c)
            i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            while i < n and text[i] != "\n":
                i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            i += 2
            while i + 1 < n and not (text[i] == "*" and text[i + 1] == "/"):
                i += 1
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out)

def strip_trailing_commas(text: str) -> str:
    out = []
    i = 0
    n = len(text)
    in_string = False
    while i < n:
        c = text[i]
        if in_string:
            out.append(c)
            if c == "\\" and i + 1 < n:
                out.append(text[i + 1])
                i += 2
                continue
            if c == '"':
                in_string = False
            i += 1
            continue
        if c == '"':
            in_string = True
            out.append(c)
            i += 1
            continue
        if c == ",":
            j = i + 1
            while j < n and text[j].isspace():
                j += 1
            if j < n and text[j] in "}]":
                i += 1
                continue
        out.append(c)
        i += 1
    return "".join(out)

user_config_path, provider_id, model_alias, model_spec, plugin_path, out_path = sys.argv[1:7]

with open(user_config_path) as f:
    raw = f.read()
try:
    user_config = json.loads(raw)
except json.JSONDecodeError:
    user_config = json.loads(strip_trailing_commas(strip_jsonc(raw)))

providers = user_config.get("provider") or {}
if provider_id not in providers:
    print(
        f"error: provider '{provider_id}' not found in {user_config_path} "
        f"(available: {', '.join(sorted(providers)) or 'none'})",
        file=sys.stderr,
    )
    sys.exit(1)

provider_block = providers[provider_id]
provider_models = provider_block.get("models") or {}
if provider_models and model_alias not in provider_models:
    print(
        f"error: model '{model_alias}' not defined under provider '{provider_id}' "
        f"(available: {', '.join(sorted(provider_models))[:300]})",
        file=sys.stderr,
    )
    sys.exit(1)

config = {
    "$schema": "https://opencode.ai/config.json",
    "autoupdate": False,
    "share": "disabled",
    "permission": {"*": "allow", "skill": {"*": "allow"}},
    "disabled_providers": ["opencode"],
    "plugin": [plugin_path],
    "provider": {provider_id: provider_block},
}
if provider_models:
    config["model"] = model_spec

with open(out_path, "w") as f:
    json.dump(config, f, indent=2)
PY
chmod 0600 "$SANDBOX_CONFIG"

# Project ONLY the selected provider's auth entry (if any) into the sandbox.
if [[ -f $USER_AUTH ]]; then
  python3 - "$USER_AUTH" "$PROVIDER_ID" "$SANDBOX_AUTH" <<'PY'
import json
import sys

auth_path, provider_id, out_path = sys.argv[1:4]
with open(auth_path) as f:
    auth = json.load(f)
if not isinstance(auth, dict):
    print("error: auth.json root is not an object", file=sys.stderr)
    sys.exit(1)
entry = auth.get(provider_id)
if entry is None:
    sys.exit(0)
with open(out_path, "w") as f:
    json.dump({provider_id: entry}, f, indent=2)
PY
  if [[ -f $SANDBOX_AUTH ]]; then
    chmod 0600 "$SANDBOX_AUTH"
  fi
fi

# Pass through only the explicitly allowlisted env vars.
if [[ -n $PROVIDER_ENV_VARS ]]; then
  export AGENT_TERMINAL_PROVIDER_ENV_VARS="$PROVIDER_ENV_VARS"
fi

# Use the same static musl binary bundled into the plugin.
AGENT_TERMINAL_BIN="$REPO_ROOT/target/x86_64-unknown-linux-musl/release/agent-terminal"
if [[ ! -x $AGENT_TERMINAL_BIN ]]; then
  fail "agent-terminal binary missing after build: $AGENT_TERMINAL_BIN"
fi
BUNDLED_ZELLIJ_DIR="$RUN_DIR/node_modules/@ufoq/opencode-agent-terminal-bundle-zellij/bin/zellij"

export AGENT_TERMINAL_HOST_PATH="$PATH"
export PATH="$BUNDLED_ZELLIJ_DIR:$(dirname "$AGENT_TERMINAL_BIN"):$PATH"

export AGENT_TERMINAL_BIN
export AGENT_TERMINAL_OPENCODE_CONFIG="$SANDBOX_CONFIG"
export AGENT_TERMINAL_VERIFY_MODE="real"
export AGENT_TERMINAL_DISABLE_MODELS_FETCH=0
export AGENT_TERMINAL_SKIP_PREFLIGHT=1
PROMPT_E2E="${AGENT_TERMINAL_ENABLE_PROMPT_E2E:-1}"
if [[ $PROMPT_E2E != 1 && ${AGENT_TERMINAL_REAL_MODEL_PROBE:-0} != 1 ]]; then
  fail "prompt e2e can only be disabled with AGENT_TERMINAL_REAL_MODEL_PROBE=1"
fi
export AGENT_TERMINAL_ENABLE_PROMPT_E2E="$PROMPT_E2E"
export AGENT_TERMINAL_CLEANUP="$CLEANUP"
export AGENT_TERMINAL_RUN_PREFIX="e2e-real"
export OPENCODE_MODEL="$OPENCODE_MODEL"
export OPENCODE_RUN_FLAGS=""
export AGENT_TERMINAL_PROMPT_E2E_TIMEOUT="${AGENT_TERMINAL_PROMPT_E2E_TIMEOUT:-900}"
if [[ -f $SANDBOX_AUTH ]]; then
  export AGENT_TERMINAL_OPENCODE_AUTH="$SANDBOX_AUTH"
fi

cd "$REPO_ROOT"
if ! bash "$E2E_HARNESS"; then
  fail "e2e harness failed"
fi
