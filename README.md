# agent-terminal

`agent-terminal` gives coding agents a small, project-scoped interface for persistent and
interactive terminal jobs. Zellij provides the PTY and human-visible panes; agents work only with
logical job names.

The project contains:

- a standalone Rust CLI;
- a thin OpenCode custom-tool adapter;
- no daemon, HTTP service, persistent output log, or raw Zellij command passthrough.

## Requirements

- Rust 1.85 or newer;
- Zellij 0.44.3 or newer;
- Bun for developing the optional OpenCode adapter.

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

## OpenCode tools

`opencode/tools/terminal.ts` exports exactly six custom tools. The copy-ready OpenCode bundle also
contains `opencode/skills/agent-terminal/SKILL.md`, which teaches agents when and how to use them.

- `terminal_start`
- `terminal_read`
- `terminal_send`
- `terminal_press`
- `terminal_stop`
- `terminal_list`

Install the built binary on `PATH`, copy the tool and skill directories into OpenCode's config,
and install the pinned OpenCode plugin package beside them:

```bash
mkdir -p ~/.config/opencode
cp -R opencode/tools opencode/skills ~/.config/opencode/
(cd ~/.config/opencode && bun add --exact @opencode-ai/plugin@1.18.4)
```

Restart OpenCode after copying config-time tool or skill files.

OpenCode enforces each custom tool permission independently. `terminal_start` also checks `bash`
permission for its command and `external_directory` when `cwd` is outside the session directory.
A conservative starting policy is:

```json
{
  "permission": {
    "terminal_list": "allow",
    "terminal_read": "allow",
    "terminal_start": "ask",
    "terminal_send": "ask",
    "terminal_press": "ask",
    "terminal_stop": "ask",
    "bash": "ask",
    "external_directory": "ask"
  }
}
```

If the binary is not on `PATH`, set an explicit path:

```bash
export AGENT_TERMINAL_BIN=/absolute/path/to/agent-terminal
```

The adapter derives project scope and default working directory from OpenCode's tool context. It
uses the Git worktree when available; when OpenCode reports the filesystem root as the worktree for
a non-Git directory, it scopes jobs to `context.directory`. Its `command` argument is executed by
the user's absolute `$SHELL` with `-c`, falling back to `/bin/sh`.

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

OpenCode adapter quality gate:

```bash
cd opencode
bun install --frozen-lockfile
bun run check
```

Single adapter test:

```bash
cd opencode
bun test -t 'start maps context'
```

Real-Zellij integration tests create isolated controller sessions and remove them in test cleanup.

## License

MIT. See `LICENSE`.
