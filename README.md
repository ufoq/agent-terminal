# agent-terminal

Persistent terminal jobs for pi.

`agent-terminal` runs each job in its own Zellij pane, so a server, watcher, REPL, or
interactive command keeps running while the agent reads output, sends input, and presses
keys across turns. The agent gets a small CLI and a skill; Zellij sessions, panes, and
cleanup stay behind the interface. Jobs are scoped to the owning `pi` process, so
subagents get isolated jobs automatically.

0.2.2 supports pi only.

## Requirements

- Linux
- The bundled Zellij 0.45.1 (`-bundle-zellij` package). Other Zellij versions are unsupported;
  use the bundle when you need a known-good backend.
- pi for the agent integration

The `-bundle-zellij` packages ship a pinned Zellij binary and verify that it reports exactly
`zellij 0.45.1` when the extension loads. The extension still loads and prints a warning
if the binary is missing or reports another version, so a deliberate override keeps
working. Builds need Rust and Bun.

## Install

```sh
cargo install --path .
```

```sh
pi install npm:@ufoq/pi-agent-terminal-bundle-zellij
```

The extension puts the packaged binary on `PATH` and registers the `agent-terminal`
skill. That `PATH` change is process-local: it applies inside `pi` and its children,
not to a plain shell or `docker exec`. Outside `pi`, invoke the packaged absolute
`bin/linux-x64/agent-terminal` and add `--global` when there is no `pi` ancestor.
The `-bundle-zellij` package also ships `bin/zellij/zellij`; the slim package
requires Zellij on `PATH`. Do not install either binary system-wide.

## CLI

```text
agent-terminal [--project PATH] [--state-dir PATH] [--global] list
agent-terminal [--global] start <JOB> [--cwd PATH] -- <COMMAND...>
agent-terminal [--global] read <JOB> [--save PATH]
agent-terminal [--global] send <JOB> <TEXT>
agent-terminal [--global] press <JOB> <KEY>
agent-terminal [--global] stop <JOB>
```

- `start` creates a named job in its own pane. Everything after `--` is the command.
- `read` returns the newest 200 lines or 32 KiB of retained scrollback. With
  `--save PATH`, it also writes the complete ANSI-stripped snapshot to `PATH` and returns
  `saved` (absolute path), `saved_bytes` (exact file size), and `saved_lines` (line count,
  including a final partial line). `truncated` describes only the inline preview.
  Relative paths use the invocation directory, the parent must exist, and an existing
  destination is atomically replaced with a mode-`0600` file. A final symlink is replaced,
  not followed. The file is a snapshot, not a live tail; saved files are not removed by
  `stop`.
- `send` pastes text and submits it with Enter.
- `press` sends one key: `Enter`, `Tab`, `Up`, `Ctrl+c`, `F5`, and similar.
- `stop` closes the job. Stopping the last job removes the session.
- `list` lists jobs in the current project and scope.

Responses are one JSON object:

```json
{"status":"ok","data":{"job":"dev"}}
{"status":"ok","data":{"jobs":[{"name":"dev"}]}}
{"status":"ok","data":{"job":"dev","screen":"...","truncated":false}}
{"status":"ok","data":{"job":"dev","screen":"...","truncated":true,"saved":"/tmp/dev.log","saved_bytes":41234,"saved_lines":812}}
{"status":"ok","data":{"job":"dev"}}
{"status":"error","code":"job_not_found","message":"job \"dev\" was not found"}
```

Errors use a stable `code`: `job_exists`, `job_not_found`, `session_lost`,
`lock_busy`, `no_pi_ancestor`, `zellij_*`, or `state_io`. The `message` never
includes a runtime state path; a save error names the caller-supplied destination.

`list` normally returns only `jobs`. If the project has state but its Zellij
session is absent, an empty list also carries `"session":"lost"`, and `read`,
`send`, and `press` return `session_lost` instead of `job_not_found`. `list`
does not sweep in that case, so an immediately following `read` also reports
the absence; the next non-`list` command sweeps the state. This is an
invocation-local observation, not a job registry: it cannot prove the named job
existed, and a normal idle cleanup or failed start can produce it. `start`
rebuilds the session; `stop` remains safe. Repeated `list` calls keep the
diagnostic visible until a non-`list` command or `start` clears it.

## Reading more output

The inline `read` preview is deliberately bounded. When older output matters, save the
complete retained scrollback to a unique file and inspect it with normal shell tools:

```sh
snapshot_dir=$(mktemp -d)
agent-terminal read dev --save "$snapshot_dir/dev.log"
# use the returned data.saved path
wc -l "$snapshot_dir/dev.log"
tail -n 200 "$snapshot_dir/dev.log"
grep -m 20 -n "error" "$snapshot_dir/dev.log"
```

`truncated:true` after a successful save is normal: it describes the preview, not the
file. Re-run with the same path to atomically replace the snapshot, or use a new unique
path and the new `saved` value. A failed save leaves the previous file at that path, so
it is stale. Check that the parent exists, is writable, and has space, then retry with the
same or a different path; do not restart the job and do not drop `--save` when older output
matters. Zellij scrollback is finite, so a snapshot can begin mid-stream; for complete
history, have the job write its own log file. Saved files are not removed by `stop`.

## Lifetime and scope

- The owner is the nearest `pi` ancestor. The full `pi` ancestor chain is recorded.
- The project is the nearest `.git` ancestor, or the invocation directory.
- A job lives while its command, owning `pi` ancestor chain, and Zellij session live.
- Parent `pi`, each subagent child, and siblings have separate job namespaces.
- When a `pi` process in the owner chain exits, a keeper pane in that session removes the
  session and its jobs.
- `--global` is a worker-lifetime namespace within the current project, shared by every
  agent using that project: it survives `pi` and is removed by `stop`, after 60 seconds
  with no jobs or unknown panes, or when the worker stops.

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
- An exited job normally disappears from `list`; `read` returns `job_not_found`.
  If its session is also absent, `list` shows `"session":"lost"` and `read`
  reports `session_lost`; the next non-`list` command sweeps, so start again.
- After a `zellij_failed` timeout, run `list` or `read` before retrying `start`;
  the operation may already have succeeded.
- `lock_busy` means another command is running; retry once.
- Run from `pi`, or use `--global`, when no `pi` ancestor exists.

## Development

```sh
(cd pi && bun run build)   # builds the CLI and fetches the bundled Zellij 0.45.1
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Integration tests use and require the bundled Zellij 0.45.1; set
`AGENT_TERMINAL_TEST_ZELLIJ` only to point at another 0.45.1 binary. Lifecycle checks are run manually against a disposable worker
with a real `pi` process.

## License

MIT
