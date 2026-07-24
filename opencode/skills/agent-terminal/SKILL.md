---
name: agent-terminal
description: Operate persistent or interactive terminal jobs with terminal_start, terminal_read, terminal_send, terminal_press, terminal_stop, and terminal_list. Use for dev servers, watchers, REPLs, debuggers, interactive prompts, and commands that must survive between agent turns. Keep short foreground commands in Bash.
license: MIT
compatibility: Requires the agent-terminal binary and OpenCode custom tools from this repository.
metadata:
  project: agent-terminal
  version: "1"
---

# Agent Terminal

Use `agent-terminal` for work that needs a persistent PTY, interactive input, human-visible terminal state, or continuation across agent turns.

Use Bash for short foreground commands.

## Choose the right surface

Use the terminal tools when at least one is true:

- a server, watcher, or build must keep running while you do other work;
- the process asks questions or needs text/key input;
- you need to return to a REPL, debugger, or interactive program;
- a human may need to inspect the same terminal job.

Do not create a terminal job for a quick command that can finish in one Bash call.

## Minimal lifecycle

1. Call `terminal_list` after context loss or when ownership is uncertain.
2. Call `terminal_start` once with a short descriptive job name.
3. Call `terminal_read` before sending input and whenever you need state or visible output.
4. Use `terminal_send` for literal text. It submits Enter by default.
5. Use `terminal_press` for named keys such as `Enter`, `Down`, or `Ctrl+D`.
6. Call `terminal_stop` when the job is no longer needed.

A successful `terminal_stop` response is authoritative cleanup confirmation. Use `terminal_list` only after context loss or when ownership is uncertain.

Do not repeatedly poll `terminal_read`. Continue useful work and read again when new state matters.

## Tool contract

| Tool | Use |
|---|---|
| `terminal_start` | Start one persistent job. Required: `job`, `command`. Optional: `cwd`. |
| `terminal_read` | Return authoritative lifecycle state plus the bounded visible screen. |
| `terminal_send` | Send literal text. Set `submit=false` only when Enter must not follow. |
| `terminal_press` | Send canonical named keys to a running job. |
| `terminal_stop` | Gracefully stop and clean up. Use `force=true` only after a graceful stop reports `job_still_running`. |
| `terminal_list` | Recover known jobs and inspect project-scoped lifecycle state. |

Job names must match `[a-z0-9][a-z0-9._-]{0,63}`. Prefer names such as `dev-server`, `tests-watch`, or `python-repl`.

## Interpret state correctly

- `running`: the job accepts input.
- `exited`: the process finished; inspect `exit_code` and the screen, then stop it to clean up.
- `lost`: persisted ownership no longer maps to a live terminal; stop it to clean stale state.

The `screen` field is a bounded visible terminal snapshot, not a complete stdout/stderr log. Respect `truncated` and application-specific log files.

## Recover from expected errors

- `job_exists`: read or stop the existing job instead of starting another copy.
- `job_not_found`: call `terminal_list`; the name or project scope may be wrong.
- `job_not_running`: read the job before sending more input.
- `job_still_running`: inspect it, then call `terminal_stop` with `force=true` only when forced pane closure is intended.
- `lock_busy`: do useful work and retry once later; do not spin.
- backend/state errors: report the exact code and message instead of guessing.

## Safety

- Read a job before sending text or keys.
- Use `terminal_stop`, not a raw `Ctrl+C` keypress, for lifecycle cleanup.
- Never put secrets in job names, commands, or sent text.
- Do not reason about sessions, panes, numeric IDs, or backend CLI flags.
- Do not claim every descendant OS process was killed. Forced stop guarantees terminal closure and controller-state cleanup.

## Examples

Persistent server:

```text
terminal_start:
  job: dev-server
  command: npm run dev
terminal_read:
  job: dev-server
terminal_stop:
  job: dev-server
```

Interactive prompt:

```text
terminal_start:
  job: repl
  command: python
terminal_read:
  job: repl
terminal_send:
  job: repl
  text: 2 + 2
terminal_read:
  job: repl
terminal_stop:
  job: repl
```

Named keys:

```text
terminal_read:
  job: debugger
terminal_press:
  job: debugger
  keys: [Down, Enter]
terminal_read:
  job: debugger
```
