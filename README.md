# agent-terminal

Persistent terminal jobs for pi.

`agent-terminal` runs each job in its own Zellij pane, so a server, watcher, REPL, or
interactive command keeps running while the agent reads output, sends input, and presses
keys across turns. The agent gets a small CLI and a skill; Zellij sessions, panes, and
cleanup stay behind the interface. Jobs are scoped to the owning `pi` process, so
subagents get isolated jobs automatically.

0.2.0 supports pi only.

## Requirements

- Linux
- Zellij 0.44 or newer
- pi for the agent integration

The `-bundle-zellij` packages ship a pinned Zellij binary. Builds need Rust and Bun.

## Install

```sh
cargo install --path .
```

```sh
pi install npm:@ufoq/pi-agent-terminal-bundle-zellij
```

The extension puts the packaged binary on `PATH` and registers the `agent-terminal`
skill.

## CLI

```text
agent-terminal [--project PATH] [--state-dir PATH] [--global] list
agent-terminal [--global] start <JOB> [--cwd PATH] -- <COMMAND...>
agent-terminal [--global] read <JOB>
agent-terminal [--global] send <JOB> <TEXT>
agent-terminal [--global] press <JOB> <KEY>
agent-terminal [--global] stop <JOB>
```

- `start` creates a named job in its own pane. Everything after `--` is the command.
- `read` returns the newest 200 lines or 32 KiB of the visible screen.
- `send` pastes text and submits it with Enter.
- `press` sends one key: `Enter`, `Tab`, `Up`, `Ctrl+c`, `F5`, and similar.
- `stop` closes the job. Stopping the last job removes the session.
- `list` lists jobs in the current project and scope.

Responses are one JSON object:

```json
{"status":"ok","data":{"job":"dev"}}
{"status":"ok","data":{"jobs":[{"name":"dev"}]}}
{"status":"ok","data":{"job":"dev","screen":"...","truncated":false}}
{"status":"ok","data":{"job":"dev"}}
{"status":"error","code":"job_not_found","message":"job \"dev\" was not found"}
```

Errors use a stable `code`: `job_exists`, `job_not_found`, `lock_busy`,
`no_pi_ancestor`, `zellij_*`, or `state_io`. The `message` never includes a state path.

## Lifetime and scope

- The owner is the nearest `pi` ancestor. The full `pi` ancestor chain is recorded.
- The project is the nearest `.git` ancestor, or the invocation directory.
- A job lives while its owning `pi` process and Zellij session live.
- Parent `pi`, each subagent child, and siblings have separate job namespaces.
- When an owner exits, a keeper pane in that session removes the session and its jobs.
- `--global` is a worker-lifetime namespace: it survives `pi`, is shared, and is removed
  by `stop`, after 60 seconds with no jobs, or when the worker stops.

Jobs must stay in their pane. Do not `nohup`, `setsid`, `tmux`, or otherwise daemonize a
job; use `--global` for long-lived work.

## State

Runtime state is ephemeral:

```text
$XDG_RUNTIME_DIR/agent-terminal   (or /tmp/agent-terminal-<uid>)
├── locks/<project>.lock
├── scratch/<project>/
└── scopes/<scope>/projects/<project>/
    ├── owners
    ├── ready
    ├── config.kdl
    └── layout.kdl
```

There is no home state and no job registry. Panes are the registry. A single project lock
serializes commands and survives cleanup. Zellij writes its config, data, and cache under
the private runtime directory, never under the user's real config.

## Recovery

- `read` before resending.
- `stop` a job when done; the last stop removes the session.
- An exited job disappears from `list`; `read` returns `job_not_found`.
- `lock_busy` means another command is running; retry once.
- Run from `pi`, or use `--global`, when no `pi` ancestor exists.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Lifecycle checks are run manually against a disposable worker with a real `pi` process.

## License

MIT
