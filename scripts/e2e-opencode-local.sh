#!/usr/bin/env bash
#
# Fully automated local-model e2e gate for agent-terminal.
#
# This script provisions everything needed to run the OpenCode prompt e2e
# against LFM2-700M served by a pinned llama.cpp binary:
#
#   - downloads/caches the LFM2-700M Q4_K_M GGUF (with SHA-256 check)
#   - downloads/caches the pinned llama.cpp b9981 release archive
#   - builds and packs the local @ufoq/opencode-agent-terminal-bundle-zellij npm package
#   - installs the packed plugin into an isolated temp environment
#   - starts llama-server on an ephemeral loopback port
#   - generates a scoped opencode.json pointing at the local model and plugin
#   - runs scripts/e2e-opencode.sh
#
# No API keys, no external litellm proxy, and no user configuration are required.
# The only assumed host tools are: bash, curl, tar, python3, bun, npm, opencode.
#
# Usage:
#   bash scripts/e2e-opencode-local.sh
#
# Environment overrides:
#   AGENT_TERMINAL_E2E_CACHE_DIR   - persistent cache for model and llama.cpp (default: ~/.cache/agent-terminal/e2e-local)
#   AGENT_TERMINAL_LFM2_URL        - override LFM2-700M GGUF URL
#   AGENT_TERMINAL_LLAMA_URL       - override llama.cpp archive URL
#   AGENT_TERMINAL_THREADS           - llama.cpp thread count (default: number of online CPUs, capped at 4)
#   AGENT_TERMINAL_CLEANUP           - delete temp dirs on exit (default: 1)
#   AGENT_TERMINAL_SKIP_PREFLIGHT    - skip download/build preflight (default: 0)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly E2E_HARNESS="$REPO_ROOT/scripts/e2e-opencode.sh"
readonly NPM_ROOT="$REPO_ROOT/opencode/npm"
readonly BUNDLE_PACKAGE="$NPM_ROOT/packages/opencode-agent-terminal-bundle-zellij"

# Pinned artifacts for the local-model release gate.
LFM2_URL="${AGENT_TERMINAL_LFM2_URL:-https://huggingface.co/LiquidAI/LFM2-700M-GGUF/resolve/main/LFM2-700M-Q4_K_M.gguf}"
LFM2_SHA256="${AGENT_TERMINAL_LFM2_SHA256:-684e8406dc13321452b3f6aeca432776e2a6a7e1ad6c23f7887b8fe3efbe2efa}"
LFM2_FILENAME="$(basename "$LFM2_URL" .gguf).gguf"

LLAMA_URL="${AGENT_TERMINAL_LLAMA_URL:-https://github.com/ggml-org/llama.cpp/releases/download/b9981/llama-b9981-bin-ubuntu-x64.tar.gz}"
LLAMA_SHA256="${AGENT_TERMINAL_LLAMA_SHA256:-bcf220f3e5de408a27315daa1ff3e84386a0dd003b48cfe7efcd6a49abbad220}"
LLAMA_ARCHIVE="$(basename "$LLAMA_URL")"
LLAMA_DIR_NAME="llama-b9981-bin-ubuntu-x64"

# Runtime settings.
THREADS="${AGENT_TERMINAL_THREADS:-}"
if [[ -z $THREADS ]]; then
  THREADS="$(nproc)"
  if [[ $THREADS -gt 4 ]]; then
    THREADS=4
  fi
  if [[ $THREADS -lt 1 ]]; then
    THREADS=1
  fi
fi

CLEANUP="${AGENT_TERMINAL_CLEANUP:-1}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"

CACHE_DIR="${AGENT_TERMINAL_E2E_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/agent-terminal/e2e-local}"

RUN_DIR=""
SERVER_PID=""
SERVER_LOG=""

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

cleanup() {
  local exit_status=$?
  trap - EXIT INT TERM HUP
  set +e

  if [[ -n $SERVER_PID ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
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
for tool in bun npm opencode curl tar python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing+=("$tool")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "missing required tools: ${missing[*]}"
fi

mkdir -p "$CACHE_DIR"

sha256_file() {
  sha256sum "$1" | awk '{print $1}'
}

download_if_missing() {
  local url="$1"
  local expected="$2"
  local dest="$3"
  local partial="$dest.partial"

  if [[ -f $dest ]]; then
    if [[ $(sha256_file "$dest") == "$expected" ]]; then
      printf 'Using cached %s\n' "$(basename "$dest")"
      return 0
    fi
    printf 'Cached file hash mismatch, re-downloading %s\n' "$(basename "$dest")" >&2
    rm -f "$dest"
  fi

  printf 'Downloading %s ...\n' "$(basename "$dest")"
  rm -f "$partial"
  if ! curl -fsSL --retry 3 -C - "$url" -o "$partial"; then
    rm -f "$partial"
    fail "failed to download $url"
  fi

  local actual
  actual="$(sha256_file "$partial")"
  if [[ $actual != "$expected" ]]; then
    rm -f "$partial"
    fail "hash mismatch for $(basename "$dest"): expected $expected, got $actual"
  fi

  mv "$partial" "$dest"
}

if [[ $SKIP_PREFLIGHT != 1 ]]; then
  download_if_missing "$LFM2_URL" "$LFM2_SHA256" "$CACHE_DIR/$LFM2_FILENAME"
  download_if_missing "$LLAMA_URL" "$LLAMA_SHA256" "$CACHE_DIR/$LLAMA_ARCHIVE"
fi

LFM2_MODEL="$CACHE_DIR/$LFM2_FILENAME"
LLAMA_EXTRACT_DIR="$CACHE_DIR/$LLAMA_DIR_NAME"
if [[ ! -f $LFM2_MODEL ]]; then
  fail "LFM2 model not found in cache: $LFM2_MODEL"
fi
if [[ ! -d $LLAMA_EXTRACT_DIR/llama-b9981 ]]; then
  printf 'Extracting %s ...\n' "$LLAMA_ARCHIVE"
  rm -rf "$LLAMA_EXTRACT_DIR"
  mkdir -p "$LLAMA_EXTRACT_DIR"
  tar -xzf "$CACHE_DIR/$LLAMA_ARCHIVE" -C "$LLAMA_EXTRACT_DIR"
fi

LLAMA_BIN_DIR="$LLAMA_EXTRACT_DIR/llama-b9981"
LLAMA_SERVER="$LLAMA_BIN_DIR/llama-server"
if [[ ! -x $LLAMA_SERVER ]]; then
  fail "llama-server binary is not executable: $LLAMA_SERVER"
fi

# Build and pack the local bundle-zellij npm package.
printf 'Building local npm plugin package ...\n'
cd "$REPO_ROOT/opencode"
if ! bun run build; then
  fail "bun run build failed"
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

# Find a free loopback port for llama-server.
SERVER_PORT="$(python3 - "$RUN_DIR" <<'PY'
import socket, sys
for port in range(18000, 19000):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if s.connect_ex(("127.0.0.1", port)) != 0:
            print(port)
            sys.exit(0)
print("", file=sys.stderr)
sys.exit(1)
PY
)"
if [[ -z $SERVER_PORT ]]; then
  fail "could not find a free port for llama-server"
fi

readonly MODEL_ALIAS="lfm2-700m-q4km"
readonly PROVIDER_NAME="local-llama"
readonly BASE_URL="http://127.0.0.1:$SERVER_PORT/v1"

SERVER_LOG="$RUN_DIR/llama-server.log"
printf 'Starting llama-server on port %s (threads=%s) ...\n' "$SERVER_PORT" "$THREADS"
LD_LIBRARY_PATH="$LLAMA_BIN_DIR:${LD_LIBRARY_PATH:-}" \
  "$LLAMA_SERVER" \
  --model "$LFM2_MODEL" \
  --alias "$MODEL_ALIAS" \
  --port "$SERVER_PORT" \
  --host 127.0.0.1 \
  --ctx-size 16384 \
  --parallel 1 \
  --n-predict 4096 \
  --threads "$THREADS" \
  --threads-batch "$THREADS" \
  --jinja \
  --no-webui \
  >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

printf 'Waiting for llama-server /health ...\n'
for _ in $(seq 1 180); do
  if curl -fsS "http://127.0.0.1:$SERVER_PORT/health" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    fail "llama-server exited before becoming healthy (log: $SERVER_LOG)"
  fi
  sleep 1
done
if ! curl -fsS "http://127.0.0.1:$SERVER_PORT/health" >/dev/null 2>&1; then
  fail "llama-server did not become healthy within 180s (log: $SERVER_LOG)"
fi

# Verify the model is advertised by the server.
if ! curl -fsS "$BASE_URL/models" >/dev/null 2>&1; then
  fail "llama-server /v1/models is not reachable"
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
            "name": "Local llama.cpp",
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

# Ensure the harness inner shell can resolve agent-terminal and the bundled Zellij.
export PATH="$BUNDLED_ZELLIJ_DIR:$(dirname "$AGENT_TERMINAL_BIN"):$PATH"

# Run the existing lifecycle harness against the local model and plugin.
export AGENT_TERMINAL_BIN
export AGENT_TERMINAL_OPENCODE_CONFIG="$CONFIG_PATH"
export OPENCODE_MODEL="$PROVIDER_NAME/$MODEL_ALIAS"
export AGENT_TERMINAL_SKIP_PREFLIGHT=1
export AGENT_TERMINAL_ENABLE_PROMPT_E2E=1
export AGENT_TERMINAL_CLEANUP=1
export AGENT_TERMINAL_RUN_PREFIX="e2e-test-repository"
export AGENT_TERMINAL_E2E_CACHE_DIR="$CACHE_DIR"

cd "$REPO_ROOT"
if ! bash "$E2E_HARNESS"; then
  fail "e2e harness failed"
fi