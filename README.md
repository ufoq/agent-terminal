# agent-terminal

`agent-terminal` gives coding agents a small, project-scoped interface for persistent and
interactive terminal jobs. Zellij provides the PTY and human-visible panes; agents work only with
logical job names.

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

Use `--project <path>` to select the project scope. It defaults to the current directory. Set
`AGENT_TERMINAL_STATE` or pass `--state-dir <path>` to override the operating-system state
directory.

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

The CLI defaults `--project` to the invocation directory and `start` defaults `--cwd` to the
project root. This skill passes an explicit stable `--project` on every call and passes `--cwd`
when the intended working directory differs from the project root, so CLI behavior does not depend
on the model's transient current directory inside the Bash tool.

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
