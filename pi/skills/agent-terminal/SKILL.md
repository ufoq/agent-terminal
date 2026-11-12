---
name: agent-terminal
description: "Run persistent or interactive terminal jobs in named Zellij panes. Use for servers, watchers, REPLs, debuggers, and anything that must survive across turns. Prefer plain Bash for short foreground commands."
compatibility: Requires the agent-terminal binary on PATH inside pi, or its packaged absolute path outside pi, and a Bash tool.
metadata:
  project: agent-terminal
  version: "3"
---

# Agent Terminal

`agent-terminal` runs commands in named, persistent terminal panes. Use it when a process must outlive one Bash call or needs interactive input. Use plain Bash when the command finishes in one call.

The project is the nearest `.git` ancestor of the invocation directory, or the directory itself. Jobs are scoped to the nearest `pi` process, so subagents get their own jobs automatically. You do not manage scopes.

The extension puts `agent-terminal` on `PATH` only inside `pi` and its children. Outside `pi` (for example a plain shell or `docker exec`), use the package's absolute `bin/linux-x64/agent-terminal` and add `--global` when there is no `pi` ancestor. Only the `-bundle-zellij` package includes Zellij; otherwise Zellij must already be on `PATH`.

## Commands

The interface is exactly these six commands: `start`, `read`, `send`, `press`, `stop`, `list`. `--cwd` is accepted only by `start`. Everything after `--` is the job command.

```bash
agent-terminal start dev -- npm run dev
agent-terminal start build --cwd ./packages/api -- npm test
agent-terminal start shell -- /bin/bash
```

Start once per job name. A duplicate start returns `job_exists`.

```bash
agent-terminal list
agent-terminal read dev
agent-terminal send repl '2 + 2'
agent-terminal press debugger Enter
agent-terminal stop dev
```

`send` pastes text and submits it with Enter. `press` sends one key. Keys are `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1`-`F12`, `Ctrl+<letter>`, and `Alt+<character>`.

`list` returns only jobs in the current project.

## Lifetime

A job lives as long as its Zellij session and its owning `pi` process. It survives turns and `/new`. When the owning `pi` process exits, or the last job is stopped, the session and its jobs are removed.

For a worker-lifetime process that must outlive `pi`, use `--global`:

```bash
agent-terminal --global start worker -- ./worker
agent-terminal --global list
agent-terminal --global stop worker
```

`--global` is the only shared namespace. It survives `pi` exit and is removed when stopped, when idle for 60 seconds, or when the worker stops.

## Output

Every response is one JSON object. Successful data is nested under `data`:

```json
{"status":"ok","data":{"job":"dev"}}
{"status":"ok","data":{"jobs":[{"name":"dev"}]}}
{"status":"ok","data":{"job":"dev","screen":"...","truncated":false}}
{"status":"ok","data":{"job":"dev"}}
{"status":"error","code":"job_not_found","message":"job \"dev\" was not found"}
```

`read` returns the newest 200 lines or 32 KiB of the pane scrollback. For complete logs and exit status, capture them inside the job command:

```bash
agent-terminal start build -- sh -c 'make > .agent-terminal-build.log 2>&1; echo exit:$? >> .agent-terminal-build.log'
```

Then `read` or `grep` the log file. An exited pane disappears, so its exit status must be written to a file if you need it.

## Recovery codes

- `job_exists` - read or stop the existing job.
- `job_not_found` - run `list`; the job may have exited, the project may differ, or the session may still be live.
- `session_lost` - the project has state but its terminal session is absent. `list` shows `"session":"lost"` and `read`/`send`/`press` return this code. A `list` that reports loss does not sweep, so a following `read` reports it too; the next non-`list` command clears the state. Call `start` again unless you need the diagnostic. This is an observation, not proof the named job existed.
- `lock_busy` - another operation is in progress; retry once later.
- `no_pi_ancestor` - run from `pi`, or use `--global`.
- `zellij_not_found`, `zellij_failed`, `state_io` - resolve the terminal or runtime problem before retrying. After a timeout, run `list` or `read` before retrying `start`; the operation outcome is unknown and may already have succeeded.

## Rules

1. `read` before resending when you do not know the prompt.
2. `stop` a job when you are done with it. The last stop removes the session.
3. A job command must stay in its pane. Do not use `nohup`, `setsid`, `tmux`, or daemonize it; use `--global` for long-lived work.
4. Do not run programs that rewrite the terminal title (`tmux`, `screen`, prompt themes that set the title). The job is found by its pane title and would become unreachable.
5. Do not background children from an interactive shell or REPL inside a job (`cmd &` after the job command has started). Only the job command itself may background a child.
6. An exited job is normally gone from `list` and `read` returns `job_not_found`. If the session is also absent, `list` shows `session:"lost"` and `read`/`send`/`press` report `session_lost`; the next non-`list` command clears the state. Start the job again when ready. Read output before a short command exits.
7. Use `list` after context loss, cancellation, or when ownership is unclear.
8. Do not replay a cancelled `send` or `press`; read first.

## Why not raw Zellij?

Raw Zellij exposes session names, pane IDs, and unstructured screen text. `agent-terminal` provides stable job names, a small JSON interface, bounded screen output, project scoping, and automatic cleanup.
