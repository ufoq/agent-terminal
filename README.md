# agent-terminal

`agent-terminal` gives coding agents a small, project-scoped interface for persistent and
interactive terminal jobs. Zellij provides the PTY and human-visible panes; agents work only with
logical job names. The wrapper adds ownership reconciliation, atomic state, and lifecycle recovery
so the agent does not have to track Zellij sessions or pane IDs.

The project contains:

- a standalone Rust CLI;
- an OpenCode npm plugin that bundles the CLI and skill;
- no daemon, HTTP service, persistent output log, or raw Zellij command passthrough.

## Requirements

- Linux x86_64;
- Zellij 0.44.3 or newer is required by the source build and by the slim npm plugin
  (`@ufoq/opencode-agent-terminal`);
- The bundle variant (`@ufoq/opencode-agent-terminal-bundle-zellij`) includes its own Zellij;
- Rust 1.85 or newer for source builds;
- Bun is required for running the OpenCode skill contract tests and quality gate.

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

## Lifecycle

Public states are deliberately small:

- `running`
- `exited`, with an optional exit code
- `lost`, when persisted ownership no longer maps to a live pane

Exited panes are held so their visible screen and exit status remain readable. `stop` closes the
pane and removes the job. A graceful stop sends the terminal `Ctrl+C` key sequence and waits up to
five seconds; if the job remains active, retry with `--force`.

Screen reads are ANSI-stripped and bounded to the newest 200 lines and 32 KiB. They represent the
visible terminal screen, not a canonical stdout/stderr log.

## End-to-end testing

`scripts/e2e-opencode-local.sh` is a fully automated, deterministic release gate. It runs as the
invoking user (no root or separate Unix account), starts a local OpenAI-compatible **fixture**
server that drives the 9-step agent-terminal lifecycle deterministically, installs the locally
built bundle plugin into an isolated OpenCode config, and exercises a real Zellij server
end-to-end via `opencode run`.

The fixture replaces the LLM in the loop: instead of a model that must *decide* to call the
Bash tool, the fixture emits the exact next `bash` tool call, validates each step's real
agent-terminal JSON output, and advances through the lifecycle. This makes the gate fast
(seconds, not minutes), deterministic (no model flakiness, no downloads), and still proves the
full integration: real OpenCode, real plugin hooks, real Bash execution, and the real
agent-terminal binary against a live Zellij server.

The gate depends on the packaged plugin, not on manual setup:

- The plugin's `config` hook must register the bundled skill — the harness asserts
  `opencode debug skill` lists `agent-terminal` before running the prompt phase.
- The plugin's `shell.env` hook must expose the binaries — the prompt phase runs with a PATH
  that excludes the bundled directories, so `agent-terminal` and Zellij resolve only through
  the plugin hook. If either hook breaks, the gate fails.

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
6. Verifies the transcript with the strict verifier (9 ordered Bash `tool_use` events with
   matching JSON payloads, no error events, no extraneous activity) and cleans up.

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
  binaries) so the plugin's `shell.env` hook is the only binary source.
- `AGENT_TERMINAL_CLEANUP` — set to `1` to delete the sandbox after the run.

Artifacts are retained under `/tmp/agent-terminal-e2e-fixture-<pid>/` unless
`AGENT_TERMINAL_CLEANUP=1` is set.

This is configuration isolation, not a security sandbox: the fixture still drives real Bash
execution as the invoking user. Run it only on throwaway machines or isolated CI runners.

For GitHub Actions, the full gate fits comfortably in the 10-minute budget on a standard
Linux runner (warm-cache `bun run release:check` completes in roughly a minute); the Rust
quality gate (`cargo fmt --check`, `clippy --all-targets -D warnings`, `cargo test
--all-targets`) dominates the runtime and can be run in parallel with the OpenCode gate.

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

Real-Zellij integration tests create isolated controller sessions and remove them in test cleanup.

## License

MIT. See `LICENSE`.
