# agent-terminal

`agent-terminal` gives coding agents a small, project-scoped interface for persistent and
interactive terminal jobs. Zellij provides the PTY and human-visible panes; agents work only with
logical job names. The wrapper adds ownership reconciliation, atomic state, and lifecycle recovery
so the agent does not have to track Zellij sessions or pane IDs.

The project contains:

- a standalone Rust CLI;
- a pi/omp coding agent extension that bundles the CLI and skill;
- no daemon, HTTP service, persistent output log, or raw Zellij command passthrough.

## Requirements

- Linux x86_64;
- Zellij 0.44.3 or newer for the source build and the slim npm packages;
- the bundle variants include their own pinned Zellij binary;
- Rust 1.85 or newer for source builds;
- Bun for the pi/omp extension tests and quality gates.

## Install

```bash
cargo install --path .
agent-terminal --version
```

The controller finds `zellij` on `PATH`. When a bundle package is used, it prepends a bundled
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

## pi / omp skill

Four npm package artifacts are built, one slim and one Zellij-bundling variant per host:

- `@ufoq/pi-agent-terminal` — bundles the static Linux x86_64 `agent-terminal` binary and the
  skill. The host must have Zellij on `PATH`.
- `@ufoq/pi-agent-terminal-bundle-zellij` — same as above, plus a pinned Zellij binary so no
  host install is required.
- `@ufoq/omp-agent-terminal` — the omp variant of the slim package.
- `@ufoq/omp-agent-terminal-bundle-zellij` — the omp variant with the pinned Zellij binary.

The two pi packages are published to the npm registry. The two omp packages are built locally but
not yet published; load them from the local package root until publication.

Each package ships a per-host adapter extension plus the bundled binary and skill. Pi manifests
declare both resources (`"pi": { "extensions": ["./dist/index.js"], "skills": ["./skills"] }`); OMP
manifests declare only the extension entry point, and omp's native `omp-plugins` provider discovers
the shipped `skills/` sibling when the package root loads. Load pi packages with `pi install
npm:@ufoq/pi-agent-terminal` (append `-l` for project-local settings), or for one run with `pi -e
npm:@ufoq/pi-agent-terminal`. Pi's extension auto-load directory is extension-only and does not read
the package's `pi.skills` metadata. Load omp packages via omp's `-e` flag or auto-discovery from
`~/.omp/agent/extensions/`.

### pi adapter

The pi adapter does not touch the bash tool at all: pi's native bash keeps its stock definition, and
the extension only

- mutates `process.env.PATH` at load time so the bundled binary directories (`bin/linux-x64` and,
  for the bundle variant, `bin/zellij`) are ordered ahead of pi's managed `~/.pi/agent/bin`;
- registers the compatibility flags `--cwd` and `--no-lsp`, which pi does not provide natively.

The bundled skill is discovered statically through the package's `pi.skills` manifest entry
(`["./skills"]`), so the adapter registers no skill handler. Loading the package root (`-e <package
root>` or `npm:`) discovers both the declared extension and the skill; loading the dist file
directly (`-e <package>/dist/index.js`) is extension-only.

Per-session isolation is native: pi injects `PI_SESSION_ID` into every bash child, and the Rust CLI
falls back to `AGENT_TERMINAL_SCOPE` → `PI_SESSION_ID` → `standalone`. No extension code is
involved, so pi gets per-session isolation even when the extension is not loaded.

### omp adapter

omp's native bash tool is not replaced either: the adapter registers a `tool_call` input-revision
handler that adjusts the arguments bash executes with, preserving the native schema, approval gate,
concurrency, and `async` conditionality. For each `bash` tool call it

- defaults `AGENT_TERMINAL_SCOPE` to the omp session id when the call carries no explicit value;
- prepends the bundled binary directories to the bash `env.PATH`;
- registers the compatibility flag `--no-context-files`, which omp does not provide natively
  (`--no-lsp` and `--cwd` are native to omp and are not registered).

The skill is discovered statically through omp's `omp-plugins` provider when the package root is
loaded.

Use the package root, not the dist file:

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
no daemon left behind.

Screen reads are ANSI-stripped and bounded to the newest 200 lines and 32 KiB. They represent the
visible terminal screen, not a canonical stdout/stderr log.

## Cross-agent isolation

Each pi/omp session gets its own invisible scope so concurrent agents never interfere. State and
Zellij sockets are isolated per scope. Pi uses its native `PI_SESSION_ID`; omp's adapter defaults
`AGENT_TERMINAL_SCOPE` to the omp session id. The model uses only job names and does not manage
scoping. The variable is optional and overridable: to share terminal state across agents (for
example a parent and subagent on the same task), each sets it to the same task-specific value such
as `20260803-fix-auth-refactor`. When running the CLI directly without a plugin, the scope defaults
to `standalone`.

## End-to-end testing

`scripts/e2e-pi-local.sh` is a fully automated, deterministic gate for pi/omp. It runs as the
invoking user, starts a local OpenAI-compatible **fixture** server that drives the agent-terminal
lifecycle deterministically, builds the locally bundled package for the agent under test, and
exercises a real Zellij server end-to-end through the real pi/omp binary with the extension loaded
via `-e`.

The fixture replaces the LLM in the loop: instead of a model that must decide to call the Bash
tool, the fixture emits the exact next bash tool call, validates each step's real agent-terminal
JSON output, and advances through the lifecycle. The gate is fast, deterministic, and still proves
the full integration: real pi/omp, real extension hooks, real Bash execution, and the real
agent-terminal binary against a live Zellij server.

`AGENT_TERMINAL_AGENT` selects the agent under test: `pi` (default) or `omp`. Each agent uses its
own package and workspace build. The fixture provider is registered through a small provider
extension file, since pi/omp do not read a config file for custom providers.

The prompt's first steps are per-agent scope probes:

- pi: `printenv PI_SESSION_ID` must equal the transcript's session header id.
- omp: `printenv AGENT_TERMINAL_SCOPE` must equal the session header id, and an explicit override
  must win.

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

`AGENT_TERMINAL_JOB_NAME` overrides the job name used by the fixture, prompt, and verifier.

Two additional scripts cover cross-session and skill concerns:

- `scripts/e2e-two-session.sh` — starts two fixture servers with `AGENT_TERMINAL_FIXTURE_HOLD=1`,
  runs two agent sessions concurrently against one shared project directory and state root, and
  asserts each scope sees only its own job.
- `scripts/e2e-skill-discovery.sh` — asserts the skill description reaches the first provider
  request, proving static package-root discovery.

```bash
AGENT_TERMINAL_AGENT=pi bash scripts/e2e-two-session.sh
AGENT_TERMINAL_AGENT=omp bash scripts/e2e-two-session.sh
AGENT_TERMINAL_AGENT=pi bash scripts/e2e-skill-discovery.sh
AGENT_TERMINAL_AGENT=omp bash scripts/e2e-skill-discovery.sh
```

### Optional real-model e2e

`scripts/e2e-pi-real.sh` is the optional, local-only companion test. It drives the same lifecycle
through the real pi/omp binary with a real model from your own config, loading the per-agent
package. It consumes real tokens, is not part of `release:check`, and is refused under CI unless
you opt in.

```bash
AGENT_TERMINAL_AGENT=pi PI_MODEL=litellm/deepseek-v4-flash bash scripts/e2e-pi-real.sh
```

The wrapper projects only the selected provider's model config (`models.json` for pi, `models.yml`
for omp) and `auth.json` entries plus a minimal `settings.json` into a sandbox config dir
(`PI_CODING_AGENT_DIR`), and always deletes that sandbox config on exit, even on failure. Provider
credentials pass through only via an explicit env-var allowlist
(`AGENT_TERMINAL_PROVIDER_ENV_VARS`).

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
- `AGENT_TERMINAL_PROVIDER_ENV_VARS` — comma-separated allowlist of env vars passed through.
- `AGENT_TERMINAL_PROMPT_E2E_TIMEOUT` — model run timeout in seconds (default `900`).
- `AGENT_TERMINAL_ALLOW_REAL_MODEL_CI` — set to `1` to permit execution under CI.

## Releasing

`release.json` at the repository root is the single source of truth for the npm release version.

`scripts/release-version.mjs` (zero-dependency Node) keeps that file and the four npm manifests in
lockstep. The managed packages are `@ufoq/pi-agent-terminal`,
`@ufoq/pi-agent-terminal-bundle-zellij`, `@ufoq/omp-agent-terminal`, and
`@ufoq/omp-agent-terminal-bundle-zellij`.

Validate the current state:

```bash
node scripts/release-version.mjs check
```

Cut a release from the repository root in this exact order:

1. `node scripts/release-version.mjs sync <version>` — writes `release.json` and the four
   manifests.
2. `node scripts/release-version.mjs check` — re-validates the synchronized state.
3. Commit the synchronized change.
4. Create the `v<version>` tag **after** that commit.
5. Build and pack each host workspace, then pack the four package directories into a temporary
   artifact directory.
6. Publish those exact tarballs with `npm publish --access public`.

Delete the artifact directory after publishing.

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

MIT
