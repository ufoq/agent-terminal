---
name: agent-terminal
description: "Run persistent or interactive terminal jobs in named Zellij panes. Use for servers, watchers, REPLs, debuggers, and anything that must survive across turns. Prefer plain Bash for short foreground commands."
compatibility: Requires the agent-terminal binary on PATH inside pi, or its packaged absolute path outside pi, and a Bash tool.
metadata:
  project: agent-terminal
  version: "5"
---

# Agent Terminal

Use `agent-terminal` when a process must outlive one Bash call or needs interactive input. Use plain Bash when the command finishes in one call.

Jobs are named terminal panes. The project is the nearest `.git` ancestor of the invocation directory, or the directory itself. Jobs are scoped to the nearest `pi` process, so subagents get separate jobs automatically; you do not manage scopes.

## Commands

The interface is exactly six commands: `start`, `read`, `send`, `press`, `stop`, `list`. Job names match `[a-z0-9][a-z0-9._-]{0,63}`.

```bash
agent-terminal start dev -- npm run dev
agent-terminal start build --cwd ./packages/api -- sh -c 'npm test > build.log 2>&1'
agent-terminal start shell -- /bin/bash

agent-terminal list
agent-terminal read dev
agent-terminal send shell 'echo hello'
agent-terminal press shell Ctrl+c
agent-terminal stop dev
```

Everything after `--` is executed directly, without a shell; use `sh -c '...'` for pipes, redirects, or other shell syntax. `--cwd` changes the job's working directory, not its project or scope.

`start` acknowledges pane creation, not application readiness or output. A duplicate name returns `job_exists`; inspect the existing job before deciding whether to replace it. An immediate `read` may be empty or partial; read again after the command has had time to run instead of starting another copy.

`list` returns job names in the current project and scope, with no per-job status or exit codes; an empty list may also carry `"session":"lost"`. An exited job disappears from `list`.

`send` pastes text then presses Enter. `press` sends one key: `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1`-`F12`, `Ctrl+<ASCII letter>`, or `Alt+<single printable ASCII character>`.

`read` returns the newest 200 lines, capped at 32 KiB; `truncated` says whether that inline preview was cut. It is always only a preview.

### More output

When `truncated` is true and you need older output, do not call `read` again expecting more lines. Save the full ANSI-stripped retained scrollback to a unique snapshot and inspect it with Bash:

```bash
snapshot_dir=$(mktemp -d)
agent-terminal read dev --save "$snapshot_dir/dev.log"
# use the returned data.saved path
wc -l "$snapshot_dir/dev.log"
tail -n 200 "$snapshot_dir/dev.log"
grep -m 20 -n "error" "$snapshot_dir/dev.log"
```

- `saved` is absolute; `saved_bytes` and `saved_lines` describe the file. `truncated` still describes only the preview, so `truncated:true` is expected after a successful save.
- Relative destinations use your invocation directory, not the job's `--cwd`. The parent must exist. The destination is atomically replaced with a mode-0600 file; a final symlink is replaced, not followed.
- Re-run with the same path to refresh it, or use a new unique path and the new `saved` value. A failed save leaves the previous snapshot in place, so it is stale; fix the destination rather than dropping `--save` when older output matters.
- The file is a snapshot, not a live tail. `--save` still returns the bounded preview. The file is replaced, not appended, and is not removed by `stop`.
- Zellij scrollback is finite: the snapshot can begin mid-stream, and a missing `grep` match may mean evicted output, not absence. `truncated:false` does not prove the file starts at job output. For complete history, have the job write its own log file.
- Keep inspection bounded: start with `wc -l`, use `tail -n`, cap searches with `grep -m 20` or `| head`, and never `cat` a large snapshot. `saved_lines` counts a final partial line, so it can differ from `wc -l` by one.

## Output and exit status

Every command response is JSON: success uses `{"status":"ok","data":...}` and failure uses `{"status":"error","code":"...","message":"..."}`.

When a job exits, its pane disappears. Capture required logs and exit status inside the job command:

```bash
agent-terminal start build -- sh -c 'make > .agent-terminal-build.log 2>&1; echo exit:$? >> .agent-terminal-build.log'
```

Then inspect the file with the normal file tools.

## Lifetime

A job survives turns and `/new` while its command, Zellij session, and owning `pi` ancestor chain remain alive. When any `pi` process in that chain exits, or the last job is stopped with no unknown panes remaining, the session and its jobs are removed.

Use `--global` for work that must outlive `pi`:

```bash
agent-terminal --global start worker -- ./worker
agent-terminal --global list
agent-terminal --global stop worker
```

`--global` jobs are shared by every agent using the same project. Use the same mode for follow-up commands and choose a distinctive name; do not stop an unfamiliar job to resolve a collision. An empty session with no job or unknown panes may be removed by a later command, by the keeper after about 60 seconds without jobs, or when the worker stops. `stop` performs idle cleanup, and stopping an absent job is not an error.

## Recovery

- `job_exists` - inspect the existing job; if you started it and need to replace it, `stop` it first.
- `job_not_found` - run `list`; check the job name, project, and `--global` mode. The job may have exited.
- `session_lost` - state exists but its Zellij session is absent. `list` may show `"session":"lost"` and `read`/`send`/`press` return this code. This is an observation, not proof the named job existed. A repeated `list` preserves it; the next non-`list` command clears it, so a later read may return `job_not_found`. Start again when ready.
- `lock_busy` - another operation is running; retry once later.
- `no_pi_ancestor` - run inside `pi`, or use `--global`.
- `zellij_not_found`, `zellij_failed`, `state_io` - resolve the terminal or runtime problem. For a save-path `state_io`, check the destination's existing parent, permissions, and free space, then retry or use a different path; this is not a reason to restart the job. After a timeout, run `list` or `read` before retrying `start`; the operation may already have succeeded.

## Rules

1. Read before sending when the prompt is unknown. After a failed, timed-out, or cancelled `send`/`press`, read before deciding whether to retry; input may already have been delivered, and `send` may have pasted text without submitting it.
2. After context loss or an interrupted `start`, reconcile with `list`/`read`. An absent job does not prove the command never ran; check captured output or side effects before repeating non-idempotent work. If `read` is truncated, save the snapshot instead of retrying `read` for older lines.
3. Keep each job in its pane: do not use `nohup`, `setsid`, `tmux`, or daemonize; do not background children from an interactive shell or REPL; only the job command may background a child.
4. Do not run programs that rewrite the terminal title, including `screen` or title-setting prompt themes; jobs are found by pane title.
5. Stop jobs when you are done with them.

## Outside pi

The extension adds `agent-terminal` to `PATH` only inside `pi` and its children. Outside `pi`, call the packaged absolute `bin/linux-x64/agent-terminal`. The `-bundle-zellij` package ships the supported Zellij 0.45.1 as `bin/zellij/zellij`; put its directory first on `PATH` or set `AGENT_TERMINAL_ZELLIJ` to it. The slim package uses whatever Zellij you install and put on `PATH`; that version choice is yours. Add `--global` when there is no `pi` ancestor.
