#!/usr/bin/env bash
#
# Fully automated deterministic e2e gate for agent-terminal.
#
# Starts a local OpenAI-compatible fixture server that drives the 9-step
# agent-terminal lifecycle deterministically — no large model download, no
# llama.cpp. The fixture validates tool results and advances through each step.
# This gate exercises real OpenCode, real plugin hooks, real Bash execution,
# and real agent-terminal — only the model's reasoning is replaced by a
# deterministic script.
#
# Usage:
#   bash scripts/e2e-opencode-local.sh
#
# Environment overrides:
#   AGENT_TERMINAL_FIXTURE_PORT     - port for the fixture server (default: auto from 19000-19100)
#   AGENT_TERMINAL_CLEANUP          - delete temp dirs on exit (default: 1)
#   AGENT_TERMINAL_SKIP_PREFLIGHT   - skip build preflight (default: 0)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly E2E_HARNESS="$REPO_ROOT/scripts/e2e-opencode.sh"
readonly NPM_ROOT="$REPO_ROOT/opencode/npm"
readonly BUNDLE_PACKAGE="$NPM_ROOT/packages/opencode-agent-terminal-bundle-zellij"
readonly FIXTURE_SCRIPT="$REPO_ROOT/opencode/scripts/e2e-fixture.ts"

CLEANUP="${AGENT_TERMINAL_CLEANUP:-1}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"

RUN_DIR=""
FIXTURE_PID=""

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

cleanup() {
  local exit_status=$?
  trap - EXIT INT TERM HUP
  set +e

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

# Preflight: required host tools.
missing=()
for tool in bun npm opencode curl python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "missing required tools: ${missing[*]}"
fi

# Build and pack the local bundle-zellij npm package.
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

# Prepare isolated run directory and install the packed plugin.
RUN_DIR="$(mktemp -d /tmp/e2e-test-repository.XXXXXX)"
readonly RUN_DIR
mkdir -p "$RUN_DIR/.opencode" "$RUN_DIR/node_modules"

if ! npm install --prefix "$RUN_DIR" "$TARBALL_PATH" --no-save --no-audit --no-fund >/dev/null 2>&1; then
  fail "failed to install packed plugin into $RUN_DIR"
fi

PLUGIN_PATH="$RUN_DIR/node_modules/@ufoq/opencode-agent-terminal-bundle-zellij/dist/index.js"
if [[ ! -f $PLUGIN_PATH ]]; then
  fail "plugin dist missing after npm install: $PLUGIN_PATH"
fi

# Find a free loopback port for the fixture.
FIXTURE_PORT="${AGENT_TERMINAL_FIXTURE_PORT:-}"
if [[ -z $FIXTURE_PORT ]]; then
  FIXTURE_PORT="$(python3 - "$RUN_DIR" <<'PY'
import socket, sys
for port in range(19000, 19100):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if s.connect_ex(("127.0.0.1", port)) != 0:
            print(port)
            sys.exit(0)
print("", file=sys.stderr)
sys.exit(1)
PY
)"
  if [[ -z $FIXTURE_PORT ]]; then
    fail "could not find a free port for the fixture server"
  fi
fi

readonly MODEL_ALIAS="fixture"
readonly PROVIDER_NAME="local-fixture"
readonly BASE_URL="http://127.0.0.1:$FIXTURE_PORT/v1"

printf 'Starting e2e fixture on port %s ...\n' "$FIXTURE_PORT"
bun run "$FIXTURE_SCRIPT" --port "$FIXTURE_PORT" &
FIXTURE_PID=$!

# Wait for the fixture to become healthy.
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$FIXTURE_PORT/health" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$FIXTURE_PID" 2>/dev/null; then
    fail "fixture exited before becoming healthy"
  fi
  sleep 0.2
done
if ! curl -fsS "http://127.0.0.1:$FIXTURE_PORT/health" >/dev/null 2>&1; then
  fail "fixture did not become healthy within 30 checks"
fi

# Generate scoped OpenCode config inside the temp worktree.
CONFIG_PATH="$RUN_DIR/.opencode/opencode.json"
python3 - "$BASE_URL" "$MODEL_ALIAS" "$PROVIDER_NAME" "$PLUGIN_PATH" "$CONFIG_PATH" <<'PY'
import json, sys
base_url, alias, provider_name, plugin_path, out_path = sys.argv[1:6]
config = {
    "$schema": "https://opencode.ai/config.json",
    "autoupdate": False,
    "share": "disabled",
    "permission": {"*": "allow", "skill": {"*": "allow"}},
    "disabled_providers": ["opencode"],
    "plugin": [plugin_path],
    "provider": {
        provider_name: {
            "npm": "@ai-sdk/openai-compatible",
            "name": "Deterministic e2e fixture",
            "options": {
                "baseURL": base_url,
                "apiKey": "local-no-secret",
                "autoload": False,
            },
            "models": {
                alias: {
                    "name": f"{provider_name}/{alias}",
                    "tool_call": True,
                    "limit": {"context": 16384, "output": 4096},
                }
            },
        }
    },
}
with open(out_path, "w") as f:
    json.dump(config, f, indent=2)
PY
chmod 0600 "$CONFIG_PATH"

printf 'OpenCode config written to %s\n' "$CONFIG_PATH"

# Use the same static musl binary that was just bundled into the plugin.
AGENT_TERMINAL_BIN="$REPO_ROOT/target/x86_64-unknown-linux-musl/release/agent-terminal"
if [[ ! -x $AGENT_TERMINAL_BIN ]]; then
  fail "agent-terminal binary missing after build: $AGENT_TERMINAL_BIN"
fi
BUNDLED_ZELLIJ_DIR="$RUN_DIR/node_modules/@ufoq/opencode-agent-terminal-bundle-zellij/bin/zellij"

# The harness inner shell resolves agent-terminal and the bundled Zellij for the
# direct CLI smoke phase. AGENT_TERMINAL_HOST_PATH keeps the ORIGINAL PATH (no
# bundled dirs) so the OpenCode prompt phase can prove the plugin's shell.env
# hook — not a preloaded PATH — is what exposes the binaries to the Bash tool.
export AGENT_TERMINAL_HOST_PATH="$PATH"
export PATH="$BUNDLED_ZELLIJ_DIR:$(dirname "$AGENT_TERMINAL_BIN"):$PATH"

# Run the existing lifecycle harness against the fixture.
export AGENT_TERMINAL_BIN
export AGENT_TERMINAL_OPENCODE_CONFIG="$CONFIG_PATH"
export OPENCODE_MODEL="$PROVIDER_NAME/$MODEL_ALIAS"
export AGENT_TERMINAL_SKIP_PREFLIGHT=1
export AGENT_TERMINAL_ENABLE_PROMPT_E2E="${AGENT_TERMINAL_ENABLE_PROMPT_E2E:-1}"
export AGENT_TERMINAL_CLEANUP=1
export AGENT_TERMINAL_RUN_PREFIX="e2e-fixture"

cd "$REPO_ROOT"
if ! bash "$E2E_HARNESS"; then
  fail "e2e harness failed"
fi
