#!/usr/bin/env bash
#
# Fully automated deterministic e2e gate for agent-terminal through the pi
# (and omp, pi-based) coding agents.
#
# Starts a local OpenAI-compatible fixture server that drives the 9-step
# agent-terminal lifecycle deterministically — no large model download, no
# llama.cpp. The fixture validates tool results and advances through each step.
# This gate exercises a real pi/omp binary, real extension hooks (spawnHook
# scope injection + PATH exposure), real Bash execution, and real
# agent-terminal — only the model's reasoning is replaced by a deterministic
# script.
#
# Usage:
#   AGENT_TERMINAL_AGENT=pi bash scripts/e2e-pi-local.sh
#   AGENT_TERMINAL_AGENT=omp bash scripts/e2e-pi-local.sh
#
# Environment overrides:
#   AGENT_TERMINAL_AGENT          - agent under test: pi (default) or omp
#   AGENT_TERMINAL_PI_DIR         - pi package dir (default: ~/.local/pi/pkg)
#   AGENT_TERMINAL_FIXTURE_PORT   - port for the fixture server (default: auto from 19000-19100)
#   AGENT_TERMINAL_CLEANUP        - delete temp dirs on exit (default: 1)
#   AGENT_TERMINAL_SKIP_PREFLIGHT - skip build preflight (default: 0)

set -Eeuo pipefail

umask 077

readonly REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
readonly E2E_HARNESS="$REPO_ROOT/scripts/e2e-pi.sh"
readonly PI_ROOT="$REPO_ROOT/pi"
readonly BUNDLE_PACKAGE="$PI_ROOT/npm/packages/pi-agent-terminal-bundle-zellij"
readonly FIXTURE_SCRIPT="$PI_ROOT/scripts/e2e-fixture.ts"

CLEANUP="${AGENT_TERMINAL_CLEANUP:-1}"
SKIP_PREFLIGHT="${AGENT_TERMINAL_SKIP_PREFLIGHT:-0}"
AGENT_TERMINAL_AGENT="${AGENT_TERMINAL_AGENT:-pi}"

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
# skill + pinned Zellij).
if [[ $SKIP_PREFLIGHT != 1 ]]; then
  printf 'Building local pi npm package ...\n'
  cd "$PI_ROOT"
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

# Prepare isolated run directory: sandbox bin dir for the agent wrapper and
# the provider extension file.
RUN_DIR="$(mktemp -d /tmp/e2e-pi-repository.XXXXXX)"
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

# pi/omp do not read a config file for custom providers (unlike opencode's
# opencode.json). The fixture provider is registered by a small extension file
# that the agent loads with -e alongside the agent-terminal extension.
PROVIDER_EXTENSION="$RUN_DIR/provider.ts"
cat >"$PROVIDER_EXTENSION" <<EOF
export default (pi: any) => {
  pi.registerProvider("$PROVIDER_NAME", {
    baseUrl: "$BASE_URL",
    apiKey: "test",
    api: "openai-completions",
    models: [
      {
        id: "$MODEL_ALIAS",
        name: "$PROVIDER_NAME/$MODEL_ALIAS",
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

printf 'Provider extension written to %s\n' "$PROVIDER_EXTENSION"

# Use the same static musl binary that was just bundled into the package.
AGENT_TERMINAL_BIN="$REPO_ROOT/target/x86_64-unknown-linux-musl/release/agent-terminal"
if [[ ! -x $AGENT_TERMINAL_BIN ]]; then
  fail "agent-terminal binary missing after build: $AGENT_TERMINAL_BIN"
fi

# The harness inner shell resolves agent-terminal and the bundled Zellij for the
# direct CLI smoke phase. AGENT_TERMINAL_HOST_PATH keeps the ORIGINAL PATH (no
# bundled dirs) so the pi prompt phase can prove the extension's spawnHook —
# not a preloaded PATH — is what exposes the binaries to the Bash tool.
export AGENT_TERMINAL_HOST_PATH="$PATH"
export PATH="$BUNDLED_ZELLIJ_DIR:$(dirname "$AGENT_TERMINAL_BIN"):$PATH"

# Run the existing lifecycle harness against the fixture.
export AGENT_TERMINAL_BIN
export AGENT_TERMINAL_AGENT
export AGENT_TERMINAL_AGENT_BIN="$AGENT_BIN"
export AGENT_TERMINAL_EXTENSION="$EXTENSION_PATH"
export AGENT_TERMINAL_PROVIDER_EXTENSION="$PROVIDER_EXTENSION"
export AGENT_TERMINAL_SKIP_PREFLIGHT=1
export AGENT_TERMINAL_ENABLE_PROMPT_E2E="${AGENT_TERMINAL_ENABLE_PROMPT_E2E:-1}"
export AGENT_TERMINAL_CLEANUP="$CLEANUP"
export AGENT_TERMINAL_RUN_PREFIX="e2e-fixture"
export AGENT_TERMINAL_VERIFY_MODE="strict"
export AGENT_TERMINAL_PROMPT_E2E_TIMEOUT=600

cd "$REPO_ROOT"
if ! bash "$E2E_HARNESS"; then
  fail "e2e harness failed"
fi
