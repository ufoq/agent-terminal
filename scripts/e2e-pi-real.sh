#!/usr/bin/env bash
#
# Optional real-model e2e for agent-terminal through pi (LOCAL ONLY, not a
# release gate).
#
# Runs the full lifecycle through pi against a REAL model from the user's own
# pi config. Unlike scripts/e2e-pi-local.sh (the deterministic fixture
# gate), this script does not replace the model - it lets a real model discover
# the packaged skill and drive agent-terminal end to end.
#
# The sandbox never sees the user's full config: only the selected provider
# block is projected from models.json, only that provider's auth.json entry is
# copied,
# settings.json is reduced to the default provider/model, and only
# allowlisted env vars are passed through. The sandbox config is always
# deleted, even on failure.
#
# Usage:
#   PI_MODEL=litellm/ollama-cloud/deepseek-v4-flash \
#   bash scripts/e2e-pi-real.sh
#   (default config dir: ~/.pi/agent; the projected model file is models.json.)
#
# Environment:
#   AGENT_TERMINAL_PI_CONFIG         path to the user's real agent config dir
#                                    (default: ~/.pi/agent)
#   PI_MODEL                         provider/model to test (required, e.g.
#                                    litellm/ollama-cloud/deepseek-v4-flash)
#   AGENT_TERMINAL_PI_DIR            pi package dir for the wrapper
#                                    (default: ~/.local/pi/pkg)
#   AGENT_TERMINAL_PROVIDER_ENV_VARS comma-separated env vars passed through
#                                    (e.g. "OPENAI_API_KEY,ANTHROPIC_API_KEY")
#   AGENT_TERMINAL_PROMPT_E2E_TIMEOUT pi run timeout in seconds (default 900)
#   AGENT_TERMINAL_ALLOW_REAL_MODEL_CI set to 1 to allow running under CI
#   AGENT_TERMINAL_REAL_MODEL_PROBE  set to 1 to allow disabling the prompt e2e
#   AGENT_TERMINAL_CLEANUP           delete wrapper worktree on exit (default: 0)
#   AGENT_TERMINAL_SKIP_PREFLIGHT    skip the pi build preflight (default: 0)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly E2E_HARNESS="$REPO_ROOT/scripts/e2e-pi.sh"
readonly PI_ROOT="$REPO_ROOT/pi"

readonly WS_ROOT="$PI_ROOT"
readonly BUNDLE_PACKAGE="$PI_ROOT/npm/packages/pi-agent-terminal-bundle-zellij"

CLEANUP="${AGENT_TERMINAL_CLEANUP:-0}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"

RUN_DIR=""
SANDBOX_PI_DIR=""

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

cleanup() {
  local exit_status=$?
  trap - EXIT INT TERM HUP
  set +e

  # Credentials never survive: the projected sandbox config is always removed.
  if [[ -n ${SANDBOX_PI_DIR:-} && -d $SANDBOX_PI_DIR ]]; then
    rm -rf "$SANDBOX_PI_DIR"
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

PI_MODEL="${PI_MODEL:-}"
if [[ -z $PI_MODEL ]]; then
  fail "PI_MODEL is required (e.g. litellm/ollama-cloud/deepseek-v4-flash)"
fi
USER_PI_CONFIG="${AGENT_TERMINAL_PI_CONFIG:-$HOME/.pi/agent}"
if [[ ! -d $USER_PI_CONFIG ]]; then
  fail "agent config dir not found: $USER_PI_CONFIG (set AGENT_TERMINAL_PI_CONFIG)"
fi
# Capture the source before clearing the inherited selector; the harness must
# only ever see the projected sandbox dir.
unset AGENT_TERMINAL_PI_CONFIG
PROVIDER_ENV_VARS="${AGENT_TERMINAL_PROVIDER_ENV_VARS:-}"

PROVIDER_ID="${PI_MODEL%%/*}"
if [[ -z $PROVIDER_ID || $PROVIDER_ID == "$PI_MODEL" ]]; then
  fail "PI_MODEL must be in provider/model form, got: $PI_MODEL"
fi
MODEL_ID="${PI_MODEL#*/}"

# Preflight: required host tools. zellij is NOT checked here: it is provided
# by the bundled package (bin/zellij) and the shared harness resolves it after
# the bundled dir is prepended to PATH.
missing=()
for tool in bun curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "missing required tools: ${missing[*]}"
fi

# Build the local bundle-zellij npm package (Rust musl binary + extension +
# skill + pinned Zellij).
if [[ $SKIP_PREFLIGHT != 1 ]]; then
  printf 'Building local pi npm package ...\n'
  cd "$WS_ROOT"
  if ! bun run build; then
    fail "bun run build failed"
  fi
fi

EXTENSION_PATH="$BUNDLE_PACKAGE/dist/index.js"
if [[ ! -f $EXTENSION_PATH ]]; then
  fail "extension dist missing after build: $EXTENSION_PATH"
fi
BUNDLED_ZELLIJ_DIR="$BUNDLE_PACKAGE/bin/zellij"
if [[ ! -x $BUNDLED_ZELLIJ_DIR/zellij ]]; then
  fail "bundled zellij missing after build: $BUNDLED_ZELLIJ_DIR/zellij"
fi
AGENT_TERMINAL_BIN="$REPO_ROOT/target/x86_64-unknown-linux-musl/release/agent-terminal"
if [[ ! -x $AGENT_TERMINAL_BIN ]]; then
  fail "agent-terminal binary missing after build: $AGENT_TERMINAL_BIN"
fi

# Prepare isolated run directory: sandbox bin dir for the agent wrapper and
# the projected pi config dir.
RUN_DIR="$(mktemp -d /tmp/e2e-real-pi-repository.XXXXXX)"
readonly RUN_DIR
mkdir -p "$RUN_DIR/.pi" "$RUN_DIR/bin"

# Build a sandbox wrapper for pi. pi is not a standalone binary: it runs as
# `bun <package>/dist/cli.js`, so the wrapper execs bun with the package entry.
AGENT_BIN="$RUN_DIR/bin/pi"
PI_DIR="${AGENT_TERMINAL_PI_DIR:-$HOME/.local/pi/pkg}"
if [[ ! -f $PI_DIR/dist/cli.js ]]; then
  fail "pi package not found at $PI_DIR (set AGENT_TERMINAL_PI_DIR)"
fi
cat >"$AGENT_BIN" <<EOF
#!/usr/bin/env bash
exec bun "$PI_DIR/dist/cli.js" "\$@"
EOF
chmod 0755 "$AGENT_BIN"

# Project a scoped agent config into the sandbox. The user's real config is
# never copied wholesale: settings.json is regenerated, the model file keeps
# only the selected provider block (models.json), and auth.json only that
# provider's entry.
SANDBOX_PI_DIR="$RUN_DIR/.pi"
SANDBOX_SETTINGS="$SANDBOX_PI_DIR/settings.json"
SANDBOX_MODELS="$SANDBOX_PI_DIR/models.json"
SANDBOX_AUTH="$SANDBOX_PI_DIR/auth.json"

# settings.json: only the selected provider/model, and always trust the
# project so non-interactive -p mode never blocks on project trust.
python3 - "$PROVIDER_ID" "$MODEL_ID" "$SANDBOX_SETTINGS" <<'PY'
import json
import sys

provider_id, model_id, out_path = sys.argv[1:4]
with open(out_path, "w") as f:
    json.dump(
        {
            "defaultProvider": provider_id,
            "defaultModel": model_id,
            "defaultProjectTrust": "always",
        },
        f,
        indent=2,
    )
PY
chmod 0600 "$SANDBOX_SETTINGS"

# models.json: project ONLY the selected provider block. If the provider is
# not declared under the user's models.json it is either a built-in provider
# or undeclared - pi will fail with a clear error if unknown.
if [[ -f $USER_PI_CONFIG/models.json ]]; then
  python3 - "$USER_PI_CONFIG/models.json" "$PROVIDER_ID" "$SANDBOX_MODELS" <<'PY'
import json
import sys

models_path, provider_id, out_path = sys.argv[1:4]
with open(models_path) as f:
    models = json.load(f)
providers = models.get("providers") if isinstance(models, dict) else None
if isinstance(providers, dict) and provider_id in providers:
    with open(out_path, "w") as f:
        json.dump({"providers": {provider_id: providers[provider_id]}}, f, indent=2)
else:
    print(
        f"warning: provider '{provider_id}' is not declared in {models_path}; "
        "it is either a built-in provider or undeclared (pi will fail with a "
        "clear error if unknown)",
        file=sys.stderr,
    )
    with open(out_path, "w") as f:
        json.dump({"providers": {}}, f)
PY
else
  printf 'warning: no models.json in %s; provider %s is either a built-in provider or undeclared (the agent will fail with a clear error if unknown)\n' \
    "$USER_PI_CONFIG" "$PROVIDER_ID" >&2
  printf '{"providers": {}}\n' >"$SANDBOX_MODELS"
fi
chmod 0600 "$SANDBOX_MODELS"

# auth.json: project ONLY the selected provider's credential entry (if any).
if [[ -f $USER_PI_CONFIG/auth.json ]]; then
  python3 - "$USER_PI_CONFIG/auth.json" "$PROVIDER_ID" "$SANDBOX_AUTH" <<'PY'
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
chmod 0700 "$SANDBOX_PI_DIR"

# Pass through only the explicitly allowlisted env vars.
if [[ -n $PROVIDER_ENV_VARS ]]; then
  export AGENT_TERMINAL_PROVIDER_ENV_VARS="$PROVIDER_ENV_VARS"
fi

# Use the same static musl binary that was just bundled into the package.
# AGENT_TERMINAL_HOST_PATH keeps the ORIGINAL PATH (no bundled dirs) so the
# prompt phase can prove the adapter's load-time PATH mutation - not a
# preloaded PATH - exposes the binaries to the Bash tool.
export AGENT_TERMINAL_HOST_PATH="$PATH"
export PATH="$BUNDLED_ZELLIJ_DIR:$(dirname "$AGENT_TERMINAL_BIN"):$PATH"

export AGENT_TERMINAL_BIN
# Pass through an exact job name when the caller set one (the harness
# defaults to prompt-smoke-$RUN_ID otherwise).
export AGENT_TERMINAL_JOB_NAME="${AGENT_TERMINAL_JOB_NAME:-}"
export AGENT_TERMINAL_PI_BIN="$AGENT_BIN"
# Load the package root so Pi reads the package's static `pi.skills` metadata.
export AGENT_TERMINAL_EXTENSION="$BUNDLE_PACKAGE"
export AGENT_TERMINAL_PI_CONFIG_DIR="$SANDBOX_PI_DIR"
export AGENT_TERMINAL_VERIFY_MODE="real"
export AGENT_TERMINAL_SKIP_PREFLIGHT=1
PROMPT_E2E="${AGENT_TERMINAL_ENABLE_PROMPT_E2E:-1}"
if [[ $PROMPT_E2E != 1 && ${AGENT_TERMINAL_REAL_MODEL_PROBE:-0} != 1 ]]; then
  fail "prompt e2e can only be disabled with AGENT_TERMINAL_REAL_MODEL_PROBE=1"
fi
export AGENT_TERMINAL_ENABLE_PROMPT_E2E="$PROMPT_E2E"
export AGENT_TERMINAL_CLEANUP="$CLEANUP"
export AGENT_TERMINAL_RUN_PREFIX="e2e-real"
export PI_MODEL_PROVIDER="$PROVIDER_ID"
export PI_MODEL_ID="$MODEL_ID"
export AGENT_TERMINAL_PROMPT_E2E_TIMEOUT="${AGENT_TERMINAL_PROMPT_E2E_TIMEOUT:-900}"

cd "$REPO_ROOT"
if ! bash "$E2E_HARNESS"; then
  fail "e2e harness failed"
fi
