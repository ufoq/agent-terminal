# agent-terminal

`agent-terminal` gives coding agents a small, project-scoped interface for persistent and
interactive terminal jobs. Zellij provides the PTY and human-visible panes; agents work only with
logical job names. The wrapper adds ownership reconciliation, atomic state, and lifecycle recovery
so the agent does not have to track Zellij sessions or pane IDs.

The project contains:

- a standalone Rust CLI;
- an OpenCode skill that teaches Bash invocation of the CLI;
- no daemon, HTTP service, persistent output log, or raw Zellij command passthrough.

## Requirements

- Rust 1.85 or newer;
- Zellij 0.44.3 or newer;
- Bun is required for running the OpenCode skill contract tests and quality gate.

## Install

```bash
cargo install --path .
agent-terminal --version
```

The controller must be able to find `zellij` on `PATH`.

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

`opencode/skills/agent-terminal/SKILL.md` teaches the model to invoke the Rust CLI directly through
Bash. There are no adapter wrappers, no plugin package, and no dedicated `terminal_*`
permissions.

Install the built binary on `PATH`, then copy the skill directory into OpenCode's config:

```bash
mkdir -p ~/.config/opencode/skills
cp -R opencode/skills/agent-terminal ~/.config/opencode/skills/
```

Restart OpenCode after copying skill files.

### Permissions

This integration adds no dedicated `terminal_*` permissions. OpenCode's Bash authorization governs
every invocation of `agent-terminal` because the skill runs the CLI as a standard Bash command.
Structured external `workdir` and recognized filesystem commands may receive additional checks, but
the embedded `--project` and `--cwd` arguments are not independently canonicalized or authorized by
OpenCode. Bash remains unsandboxed: the model can invoke the CLI through Bash just as it would any
other command on the host.

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

`scripts/e2e-opencode.sh` runs the OpenCode skill end-to-end against a real Zellij server
as the invoking user. It does not require root or create a separate Unix account. Isolation
is achieved by redirecting OpenCode's config/data/cache/state directories into a temporary
sandbox and running `opencode --pure run` so no external plugins are loaded.

```bash
bash scripts/e2e-opencode.sh
```

Environment variables:

- `AGENT_TERMINAL_TEST_USER` — used only as a sandbox label (default `tester-e2e`).
- `AGENT_TERMINAL_ENABLE_PROMPT_E2E` — run the full `opencode run` prompt lifecycle
  (default `1`). Set to `0` to skip the LLM-driven phase and run only the skill
  contract and direct CLI smoke tests.
- `OPENCODE_MODEL` — model passed to `opencode run --model` (default
  `litellm/ollama-cloud/deepseek-v4-flash`).
- `AGENT_TERMINAL_BIN` — path to a pre-built binary; if unset, the runner builds
  `target/release/agent-terminal`.
- `AGENT_TERMINAL_OPENCODE_CONFIG` — path to a custom `opencode.json` to use instead of
  the generated minimal config.

Run from the `opencode/` directory with Bun:

```bash
cd opencode
bun run e2e:opencode
bun run e2e:opencode:skip-prompt
```

Artifacts are retained under `/tmp/agent-terminal-e2e-$AGENT_TERMINAL_TEST_USER-<pid>/`
unless `AGENT_TERMINAL_CLEANUP=1` is set.

This harness is configuration isolation, not a security sandbox: the LLM still executes Bash
as the invoking user. Run it only on throwaway machines or isolated CI runners.

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
