# agent-terminal

`agent-terminal` gives coding agents a small, project-scoped interface for persistent and
interactive terminal jobs. Zellij provides the PTY and human-visible panes; agents work only with
logical job names. The wrapper adds ownership reconciliation, atomic state, and lifecycle recovery
so the agent does not have to track Zellij sessions or pane IDs.

The project contains:

- a standalone Rust CLI;
- an OpenCode npm plugin that bundles the CLI and skill;
- a pi/omp coding agent extension that bundles the CLI and skill;
- no daemon, HTTP service, persistent output log, or raw Zellij command passthrough.

## Requirements

- Linux x86_64;
- Zellij 0.44.3 or newer is required by the source build and by the slim npm plugin
  (`@ufoq/opencode-agent-terminal`);
- The bundle variant (`@ufoq/opencode-agent-terminal-bundle-zellij`) includes its own Zellij;
- Rust 1.85 or newer for source builds;
- Bun is required for running the OpenCode skill contract tests and quality gate;
- Bun is required for running the pi/omp extension tests and quality gate.

## Install

```bash
cargo install --path .
agent-terminal --version
```

The controller finds `zellij` on `PATH`. When the bundle plugin is used, it prepends a bundled
Zellij binary to `PATH` so no host install is needed.

## CLI

All machine responses are one compact JSON object on stdout. Diagnostics go to stderr. `--help`
and `--version` are intentionally plain text.

```text
agent-terminal [OPTIONS] <COMMAND>

Commands:
  start  Start a persistent terminal job
  read   Read lifecycle state and the bounded visible screen
  send   Send literal text, followed by Enter by default
  press  Press canonical named keys
  stop   Stop and clean up a job
  list   List project-scoped jobs
```

Example:

```bash
agent-terminal start dev-server -- npm run dev
agent-terminal read dev-server
agent-terminal send repl -- '2 + 2'
agent-terminal press debugger -- Down Enter
agent-terminal stop dev-server
agent-terminal list
```

Use `--project <path>` to select the project scope. It defaults to the nearest Git root, or to the
current directory when not inside a Git repository. `start` defaults `--cwd` to the invocation
directory. Set `AGENT_TERMINAL_STATE` or pass `--state-dir <path>` to override the operating-system
state directory.

Job names must match `[a-z0-9][a-z0-9._-]{0,63}`. The same job name may be used independently in
different project roots.

## OpenCode skill

Two npm packages are published:

- `@ufoq/opencode-agent-terminal` — bundles the static Linux x86_64 `agent-terminal` binary and the
  skill. The host must have Zellij on `PATH`.
- `@ufoq/opencode-agent-terminal-bundle-zellij` — same as above, plus a pinned Zellij binary so no
  host install is required.

Add the exact package version you want to `opencode.json`:

```json
{
  "plugin": ["@ufoq/opencode-agent-terminal-bundle-zellij@0.1.2"]
}
```

Restart OpenCode after editing the config. On startup, OpenCode installs the npm package, registers
the bundled `agent-terminal` skill, and exposes the bundled binaries on Bash `PATH`.

For local development, `opencode/skills/agent-terminal/SKILL.md` can also teach the model to invoke a
locally built CLI directly through Bash.

Install the built binary on `PATH`, then copy the skill directory into OpenCode's config:

```bash
mkdir -p ~/.config/opencode/skills
cp -R opencode/skills/agent-terminal ~/.config/opencode/skills/
```

Restart OpenCode after copying skill files.

### Permissions

The skill invokes the CLI through Bash, so Bash authorization governs every `agent-terminal`
invocation. The skill does not add its own permission category. Structured external `workdir`
and recognized filesystem commands may receive additional checks, but the embedded `--project`
and `--cwd` arguments are not independently canonicalized or authorized by OpenCode. Bash remains
unsandboxed: the model can invoke the CLI through Bash just as it would any other command on the
host.

### Cancellation

In OpenCode, model actions can be cancelled mid-execution. Follow these rules:

- Cancelled `start` or `stop` must be reconciled by a same-scope `list` or `read`. A cancelled
  `start` may still have launched the job; a cancelled `stop` may not have cleaned it up.
- Never automatically replay cancelled `send` or `press`. That the command was issued does not
  prove the terminal consumed the input.
- `read` and `list` are safe to retry after cancellation.

### Project and working directory

The CLI defaults `--project` to the nearest Git root and `start` defaults `--cwd` to the invocation
directory. Most calls need no explicit `--project` or `--cwd`; use them only to override the default
scope or working directory.

## pi / omp skill

Four npm package artifacts are built, one slim and one Zellij-bundling variant per host:

- `@ufoq/pi-agent-terminal` — bundles the static Linux x86_64 `agent-terminal` binary and the
  skill. The host must have Zellij on `PATH`.
- `@ufoq/pi-agent-terminal-bundle-zellij` — same as above, plus a pinned Zellij binary so no
  host install is required.
- `@ufoq/omp-agent-terminal` — the omp variant of the slim package.
- `@ufoq/omp-agent-terminal-bundle-zellij` — the omp variant with the pinned Zellij binary.

The two pi packages are published to the npm registry. The two omp packages are built
locally but not yet published: their registry endpoints return 404 and publication is
deferred. Until they are published, load the omp packages from the local package root (see
the usage note below).

Each package ships a per-host adapter extension plus the bundled binary and skill. Pi
manifests declare both resources (`"pi": { "extensions": ["./dist/index.js"], "skills":
["./skills"] }`); OMP manifests declare only the extension entry point, and omp's native
`omp-plugins` provider discovers the shipped `skills/` sibling when the package root
loads. The pi packages are loaded via pi's `-e` flag or auto-discovered from
`~/.pi/agent/extensions/`; the omp packages via omp's `-e` flag or auto-discovered from
`~/.omp/agent/extensions/`.

### pi adapter

The pi adapter does not touch the bash tool at all: pi's native bash keeps its stock
definition, and the extension only

- mutates `process.env.PATH` at load time so the bundled binary directories (`bin/linux-x64`
  and, for the bundle variant, `bin/zellij`) are ordered ahead of pi's managed
  `~/.pi/agent/bin` — this also satisfies pi's own conditional managed-bin prepend, so the
  bundled `agent-terminal` and Zellij always win without host installs;
- registers the compatibility flags `--cwd` and `--no-lsp`, which pi does not provide
  natively. These are compatibility registrations only: the adapter makes no behavior
  claims, and `--cwd` no longer feeds a bash tool (bash's working directory is the
  invocation directory).

The bundled skill is discovered statically: the package's `pi.skills` manifest entry
(`["./skills"]`) is what exposes it, so the adapter registers no skill handler. Loading
the package root (`-e <package root>` or `npm:`) discovers both the declared extension
and the skill; loading the dist file directly (`-e <package>/dist/index.js`) is
extension-only and intentionally does not discover the skill.

Per-session isolation is native: pi injects `PI_SESSION_ID` into every bash child, and the
Rust CLI falls back to `AGENT_TERMINAL_SCOPE` → `PI_SESSION_ID` → `standalone`. No extension
code is involved, so pi gets per-session isolation even when the extension is not loaded.

### omp adapter

omp's native bash tool is not replaced either: the adapter registers a
`tool_call` input-revision handler that adjusts the arguments bash executes with, preserving
the native schema, approval gate, concurrency, and `async` conditionality by construction.
For each `bash` tool call it

- defaults `AGENT_TERMINAL_SCOPE` to the omp session id (from the runtime context's
  `sessionManager`) when the call carries no explicit value. Explicit values always win —
  including shell-level assignments such as `AGENT_TERMINAL_SCOPE=shared printenv
  AGENT_TERMINAL_SCOPE`, since bash env values are passed as environment values, not shell
  text. Without a session id (or without a session manager), the scope is left unset and the
  CLI falls back to `standalone`;
- prepends the bundled binary directories to the bash `env.PATH`. The bundled dirs are
  always prepended; the process PATH is used as the base only when the call supplies no
  `env.PATH`, and a supplied value (including an explicit empty string, which yields the
  bundled dirs alone) replaces the process PATH rather than inheriting it;
- registers the compatibility flag `--no-context-files`, which omp does not provide
  natively (`--no-lsp` and `--cwd` are native to omp and are not registered).

The skill is discovered statically: omp's `omp-plugins` provider registers
`skills/agent-terminal/SKILL.md` for packages loaded through `-e <package root>` or the
`npm:` spec, so the adapter registers no skill handler.

Known caveats:

- On direnv projects, an explicit bash `env.PATH` takes precedence over the `.envrc` PATH
  (omp's native caller-env-over-direnv precedence); the bundled binaries are always
  prepended, and the process PATH is the base only when the call supplies no `env.PATH`.
- `tool_call` input revisions compose across extensions with the last returned input
  winning, and handlers do not observe each other's revisions — another extension revising
  bash input may override this adapter's revision or vice versa.

To use it, load the package for your host with `-e`. Both hosts discover the shipped
skill only when the package root is loaded — `-e <package root>` or the `npm:` spec —
not when the dist file is loaded directly. The pi package loads from npm. The omp
package also loads from `npm:` once published; until then load it from the local package
root (the skills-sibling directory, not the dist file):

```bash
pi -e npm:@ufoq/pi-agent-terminal-bundle-zellij
omp -e npm:@ufoq/omp-agent-terminal-bundle-zellij          # post-publication
omp -e omp/npm/packages/omp-agent-terminal-bundle-zellij   # local, until published
```

## Lifecycle

Public states are deliberately small:

- `running`
- `exited`, with an optional exit code

Exited panes are held so their visible screen and exit status remain readable. `stop` closes the
pane and removes the job. Stop sends Ctrl+C, waits up to 5 seconds for the command to exit, then
force-closes the pane and removes the job. It auto-escalates; no `--force` flag is needed. When the
last job in a session is stopped, `stop` also tears down that session's Zellij server, so there is
no daemon left behind. A crashed agent's orphaned daemon (whose last `stop` never ran) can be
reclaimed by an operator with `pkill -f 'agent-terminal-<scope-digest>'` or, less precisely,
`pkill -f zellij`. Agent-driven cleanup is via `stop` only; no separate garbage-collection command
exists.

Screen reads are ANSI-stripped and bounded to the newest 200 lines and 32 KiB. They represent the
visible terminal screen, not a canonical stdout/stderr log.

## Cross-agent isolation

Each OpenCode session gets its own invisible scope so concurrent agents never interfere. The plugin
sets `AGENT_TERMINAL_SCOPE` to the session id by default; state and Zellij sockets are isolated
per-scope. The pi/omp adapters do the same by default: pi via its native `PI_SESSION_ID`
(which the Rust CLI falls back to when the extension is absent), and omp via the adapter's
`tool_call` revision defaulting `AGENT_TERMINAL_SCOPE` to the omp session id. The model uses
only job names and does
not manage scoping. The variable is optional and overridable: to share terminal state across
agents (e.g. a parent and subagent on the same task), each sets it to the same task-specific value
such as `20260803-fix-auth-refactor` — the plugin honors an explicit value unchanged and only
auto-injects the session id when unset. When running the CLI directly without a plugin, the scope
defaults to `standalone`.

## End-to-end testing

`scripts/e2e-opencode-local.sh` is a fully automated, deterministic release gate. It runs as the
invoking user (no root or separate Unix account), starts a local OpenAI-compatible **fixture**
server that drives the 10-step agent-terminal lifecycle deterministically, installs the locally
built bundle plugin into an isolated OpenCode config, and exercises a real Zellij server
end-to-end via `opencode run`.

The fixture replaces the LLM in the loop: instead of a model that must *decide* to call the
Bash tool, the fixture emits the exact next `bash` tool call, validates each step's real
agent-terminal JSON output, and advances through the lifecycle. This makes the gate fast
(seconds, not minutes), deterministic (no model flakiness, no model downloads), and still proves the
full integration: real OpenCode, real plugin hooks, real Bash execution, and the real
agent-terminal binary against a live Zellij server.

The gate depends on the packaged plugin, not on manual setup:

- The plugin's `config` hook must register the bundled skill — the harness asserts
  `opencode debug skill` lists `agent-terminal` before running the prompt phase.
- The plugin's `shell.env` hook must inject the per-session scope — the prompt phase's first
  Bash call is `printenv AGENT_TERMINAL_SCOPE`, and the verifier requires its printed value to
  exactly equal the transcript's OpenCode `sessionID`. The bundled binaries resolve on Bash
  `PATH` (factory-time PATH mutation exposes them even if `shell.env` never fired, so scope
  injection is what the probe actually guards). If either hook breaks, the gate fails.

```bash
bash scripts/e2e-opencode-local.sh
```

What it does in one call:

1. Checks prerequisites (`bun`, `npm`, `opencode`, `curl`, and `python3`).
2. Builds and packs the local `@ufoq/opencode-agent-terminal-bundle-zellij` plugin.
3. Starts the e2e fixture server on an ephemeral loopback port.
4. Writes a scoped `opencode.json` inside a temp worktree
   (`/tmp/e2e-test-repository.XXXXXX/.opencode/`).
5. Runs the shared lifecycle harness: `bun test`, direct CLI smoke, and the fixture-driven
   `opencode run` prompt phase.
6. Verifies the transcript with the strict verifier (1 scope-probe + 9 ordered Bash `tool_use`
   events with matching JSON payloads, no error events, no extraneous activity) and cleans up.

Run from `opencode/` with Bun:

```bash
cd opencode
bun run e2e:opencode
bun run e2e:opencode:skip-prompt
```

The full release gate also runs `bun run check` first:

```bash
cd opencode
bun run release:check
```

`scripts/e2e-opencode.sh` remains the shared lifecycle harness. It can still be invoked
directly with an explicit `AGENT_TERMINAL_OPENCODE_CONFIG` and `OPENCODE_MODEL` for custom
providers, but the default entry point is the deterministic fixture flow.

Environment variables:

- `AGENT_TERMINAL_FIXTURE_PORT` — port for the fixture server (default: auto from 19000).
- `AGENT_TERMINAL_ENABLE_PROMPT_E2E` — run the prompt-driven phase (default `1`).
- `AGENT_TERMINAL_BIN` — path to a pre-built binary; if unset, the wrapper builds the
  `x86_64-unknown-linux-musl` release binary.
- `AGENT_TERMINAL_HOST_PATH` — PATH used for the `opencode run` prompt phase; defaults to the
  invoking PATH. The fixture wrapper sets this to the original PATH (without the bundled
  binaries). The scope probe (not PATH) proves the `shell.env` hook fired.
- `AGENT_TERMINAL_CLEANUP` — set to `1` to delete the sandbox after the run.

By default the fixture wrapper removes temporary directories. Set
`AGENT_TERMINAL_CLEANUP=0` to retain the wrapper worktree and the harness evidence directory
under `/tmp/agent-terminal-e2e-fixture-<pid>-evidence/`.

This is configuration isolation, not a security sandbox: the fixture still drives real Bash
execution as the invoking user. Run it only on throwaway machines or isolated CI runners.

For GitHub Actions, the full gate fits comfortably in the 10-minute budget on a standard
Linux runner (warm-cache `bun run release:check` completes in roughly a minute); the Rust
quality gate (`cargo fmt --check`, `clippy --all-targets -D warnings`, `cargo test
--all-targets`) dominates the runtime and can be run in parallel with the OpenCode gate.

### pi/omp e2e gate

`scripts/e2e-pi-local.sh` is a fully automated, deterministic gate mirroring the OpenCode gate
but driving pi/omp instead. It runs as the invoking user, starts the same local
OpenAI-compatible **fixture** server that drives the agent-terminal lifecycle deterministically,
builds the locally bundled package for the agent under test, and exercises a real Zellij
server end-to-end through the real pi/omp binary with the per-agent extension loaded via `-e`.

`AGENT_TERMINAL_AGENT` selects the agent under test: `pi` (default) or `omp`. Each agent uses
its own package (`pi/npm/packages/pi-agent-terminal-bundle-zellij` for pi,
`omp/npm/packages/omp-agent-terminal-bundle-zellij` for omp) and its own workspace build; the
preflight runs the per-agent workspace `bun run build`. The fixture provider is registered
through a small provider extension file, since pi/omp do not read a config file for custom
providers the way OpenCode does.

The prompt's first steps are per-agent scope probes that verify the host's scope mechanism
end to end:

- pi: one probe, `printenv PI_SESSION_ID` — its output must equal the transcript's session
  header id (native `PI_SESSION_ID` → Rust CLI fallback).
- omp: two probes — `printenv AGENT_TERMINAL_SCOPE` (must equal the session header id, via
  the adapter's `tool_call` revision) and `AGENT_TERMINAL_SCOPE=shared printenv
  AGENT_TERMINAL_SCOPE` (must print `shared`, proving an explicit value overrides the
  injected default).

The fixture and verifier strip omp's "Wall time: X seconds" output suffix, which omp's bash
tool appends after the actual command output, so the strict JSON matching works for both
agents.

```bash
AGENT_TERMINAL_AGENT=pi bash scripts/e2e-pi-local.sh
AGENT_TERMINAL_AGENT=omp bash scripts/e2e-pi-local.sh
```

Run the per-agent gate from its own workspace with Bun:

```bash
cd pi
bun run e2e:pi
bun run e2e:pi:skip-prompt
```

```bash
cd omp
bun run e2e:omp
bun run e2e:omp:skip-prompt
```

The full release gate also runs the per-agent workspace `bun run check` first:

```bash
cd pi
bun run release:check
```

`AGENT_TERMINAL_JOB_NAME` overrides the job name used by the fixture, prompt, and verifier
(default `prompt-smoke-$RUN_ID`); the two-session isolation script uses it to keep both
concurrent runs on the same job name.

Two additional scripts cover cross-session and skill concerns beyond the lifecycle gate:

- `scripts/e2e-two-session.sh` — a deterministic two-session isolation gate for the selected
  agent. It builds once, starts two fixture servers with `AGENT_TERMINAL_FIXTURE_HOLD=1`
  (each holding its job open for `AGENT_TERMINAL_FIXTURE_HOLD_SECS`, default 45 seconds),
  and runs two agent sessions concurrently against one shared project directory, state root,
  and Zellij socket dir. It asserts the two session header ids differ, and during the hold
  window runs direct-CLI cross-checks: `agent-terminal list` under each scope sees exactly
  its own job, and `agent-terminal read server` under each scope reads its own pane. Both
  sessions then complete the full lifecycle with empty final lists.
- `scripts/e2e-skill-discovery.sh` — a unified skill-discovery smoke for both hosts. With
  `AGENT_TERMINAL_FIXTURE_SKILL_PROBE=1`, the mini fixture inspects the first provider
  request's messages and asserts the skill's DESCRIPTION phrase ("Run persistent or
  interactive terminal jobs through a simple Zellij wrapper") is present, proving static
  package-root discovery reaches the model request. Both runs load the package with
  `-e <package root>` — the package root, not the dist file — because Pi discovers the
  declared `pi.skills` directory there and omp's native `omp-plugins` provider discovers
  its shipped `skills/` sibling there.

```bash
AGENT_TERMINAL_AGENT=pi bash scripts/e2e-two-session.sh
AGENT_TERMINAL_AGENT=omp bash scripts/e2e-two-session.sh
AGENT_TERMINAL_AGENT=pi bash scripts/e2e-skill-discovery.sh
AGENT_TERMINAL_AGENT=omp bash scripts/e2e-skill-discovery.sh
```

### Optional real-model e2e

`scripts/e2e-opencode-real.sh` is an **optional, local-only** companion test that runs the same
9-step lifecycle with a **real model** from your own OpenCode configuration. It exists to prove
that an actual model (not the deterministic fixture) can discover the packaged skill and
operate the terminal end-to-end. It is not part of `release:check` and is intentionally slow
and non-deterministic — use it when you want to validate a model or a config change manually.

It is **refused automatically under CI** (when the `CI` environment variable is set) unless you
explicitly opt in with `AGENT_TERMINAL_ALLOW_REAL_MODEL_CI=1`, because a real-model test has no
place in a deterministic release gate.

```bash
cd opencode
AGENT_TERMINAL_OPENCODE_CONFIG=~/.config/opencode/opencode.json \
  OPENCODE_MODEL=litellm/ollama-cloud/deepseek-v4-flash \
  bun run e2e:opencode:real
```

What it does:

1. Parses your real `opencode.json` (JSONC-tolerant) and **projects only the selected
   provider's block** into a scoped sandbox config — together with the packed bundle plugin
   and safe permissions. Your other providers, plugins, MCP servers, agents, and instructions
   are never copied, so nothing from your personal config leaks into the test run.
2. Copies **only the selected provider's entry** from `auth.json` into the sandbox
   (default `~/.local/share/opencode/auth.json`, overridable via `AGENT_TERMINAL_OPENCODE_AUTH`),
   and passes through only an explicit env-var allowlist
   (`AGENT_TERMINAL_PROVIDER_ENV_VARS`, comma-separated) — never the whole host environment.
3. Runs the shared lifecycle harness in `real` verify mode: the same 9 ordered JSON
   postconditions and `E2E_SUCCESS`, allowing up to 8 successful retries of the current
   observational `read`/`list` milestone. The verifier is otherwise closed-world: it accepts
   only exact lifecycle command templates, the harness workdir, and completed Bash calls while
   rejecting unrelated tools, altered or compound commands, failed calls, out-of-order or
   duplicate mutations, incomplete cleanup, and top-level errors.
4. Always scrubs the projected config and auth data on exit — even on failure — retaining
   only an evidence directory. Evidence contains unsanitized model/tool output and must be
   treated as sensitive.

This isolates the OpenCode configuration inputs, not the execution environment: the selected
provider, plugin, model, and Bash commands still run with the invoking user's privileges. Use a
disposable host or isolated runner, and treat provider credentials exposed to the selected model
as active during the test.

Environment variables:

- `AGENT_TERMINAL_OPENCODE_CONFIG` — path to your real `opencode.json`/`opencode.jsonc`
  (required).
- `OPENCODE_MODEL` — `provider/model` alias that exists in that config (required).
- `AGENT_TERMINAL_OPENCODE_AUTH` — path to `auth.json` to project credentials from (default:
  OpenCode's standard location).
- `AGENT_TERMINAL_PROVIDER_ENV_VARS` — comma-separated allowlist of env vars passed through to
  the sandbox (e.g. `OPENAI_API_KEY,ANTHROPIC_API_KEY`).
- `AGENT_TERMINAL_PROMPT_E2E_TIMEOUT` — model run timeout in seconds (default `900`).
- `AGENT_TERMINAL_ALLOW_REAL_MODEL_CI` — set to `1` to permit execution under CI.
- `AGENT_TERMINAL_REAL_MODEL_PROBE` — set to `1` only for projection/auth setup probes that
  intentionally set `AGENT_TERMINAL_ENABLE_PROMPT_E2E=0`; this is not a lifecycle test.
- `AGENT_TERMINAL_CLEANUP` — set to `1` to remove the real-model wrapper worktree (default `0`,
  while projected config/auth data are always scrubbed).

#### pi/omp real-model e2e

`scripts/e2e-pi-real.sh` is the **optional, local-only** companion test for pi/omp, mirroring
`scripts/e2e-opencode-real.sh`. It drives the same lifecycle through the real pi/omp binary with
a **real model** from your own pi config, loading the per-agent package (`pi/npm/packages/
pi-agent-terminal-bundle-zellij` for pi, `omp/npm/packages/omp-agent-terminal-bundle-zellij` for
omp), and — like the OpenCode version — it consumes real
tokens, is not part of `release:check`, and is refused under CI unless you opt in. `AGENT_TERMINAL_AGENT` selects the agent under test: `pi` (default) or `omp`.

```bash
AGENT_TERMINAL_AGENT=pi PI_MODEL=litellm/deepseek-v4-flash bash scripts/e2e-pi-real.sh
```

The wrapper projects only the selected provider's model config (`models.json` for pi, `models.yml`
for omp) and `auth.json` entries plus a minimal `settings.json` into a sandbox config dir
(`PI_CODING_AGENT_DIR`), and always deletes that sandbox config on exit, even on failure. Nothing
from your personal pi config leaks into the test run, and provider credentials are passed through
only via an explicit env-var allowlist (`AGENT_TERMINAL_PROVIDER_ENV_VARS`) — never the whole host
environment.

Run from the agent's own workspace with Bun:

```bash
cd pi
bun run e2e:pi:real
```

```bash
cd omp
bun run e2e:omp:real
```

Environment variables:

- `PI_MODEL` — `provider/model` alias that exists in your pi config (required).
- `AGENT_TERMINAL_PI_CONFIG` — path to your real pi config directory (default `~/.pi/agent`).
- `AGENT_TERMINAL_PROVIDER_ENV_VARS` — comma-separated allowlist of env vars passed through to
  the sandbox.
- `AGENT_TERMINAL_PROMPT_E2E_TIMEOUT` — model run timeout in seconds (default `900`).
- `AGENT_TERMINAL_ALLOW_REAL_MODEL_CI` — set to `1` to permit execution under CI.

## Development

Rust quality gate:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo build --release
```

Single Rust test:

```bash
cargo test --test zellij_e2e start_read_stop_when_job_is_running -- --exact
```

OpenCode skill quality gate:

```bash
cd opencode
bun install --frozen-lockfile
bun run check
```

Single skill test:

```bash
cd opencode
bun test -t 'teaches CLI commands'
```

pi/omp extension quality gates:

```bash
cd pi
bun install
bun run check
```

```bash
cd omp
bun install
bun run check
```

Single pi extension test:

```bash
cd pi
bun test -t 'PATH ordering'
```

Real-Zellij integration tests create isolated controller sessions and remove them in test cleanup.

## License

MIT. See `LICENSE`.
