---
name: agent-terminal
description: "Invoke the agent-terminal CLI through Bash for persistent or interactive terminal jobs. Use for dev servers, watchers, REPLs, debuggers, and commands that must survive between agent turns. Keep short foreground commands in Bash."
compatibility: Requires the agent-terminal binary on PATH and a Bash tool. Works with OpenCode, Pi, and any host that exposes a Bash tool.
metadata:
  project: agent-terminal
  version: "1"
---

# Agent Terminal

The `agent-terminal` CLI gives you persistent, project-scoped terminal jobs through Zellij. Invoke it through
Bash for work that needs a persistent PTY, interactive input, or continuation across turns.

Use plain Bash for short foreground commands that finish in one call.

## Project and working directory

Derive a stable project root from Git or fall back to the current directory. Both variables must be
re-derived in every Bash call because they do not persist between invocations.

```bash
PROJECT="$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)"
CWD="$(pwd -P)"
```

Pass `$PROJECT` on every command to define the job scope. Pass `--cwd "$CWD"` to `start` when the job
working directory differs from `$PROJECT`. If the job should run from the project root, omit `--cwd`.

All six verbs accept these global options:

```text
--project <PATH>       Required. Project identity and state scope.
--state-dir <PATH>     Override state root (default: OS cache dir or $AGENT_TERMINAL_STATE).
--pretty               Enable pretty-printed JSON output.
-v, -vv, -vvv          Increase stderr diagnostic verbosity.
```

## Commands

### start

Start a persistent terminal job. The command follows `--` as literal argv.

```bash
agent-terminal --project "$PROJECT" start dev-server --cwd "$CWD" -- npm run dev
```

For shell syntax such as pipes, redirects, or variable expansion, wrap the command in `/bin/sh -c`:

```bash
agent-terminal --project "$PROJECT" start build -- /bin/sh -c 'make -j"$(nproc)" 2>&1 | tee build.log'
```

If `--cwd` is omitted, the job starts from the project root.

### read

Read lifecycle state and the bounded visible screen. Call before sending input and whenever you
need state or output.

```bash
agent-terminal --project "$PROJECT" read dev-server
```

### send

Send literal text to a running job. Enter follows by default. Use `--no-submit` when Enter must not
be appended.

```bash
agent-terminal --project "$PROJECT" send repl -- '2 + 2'
agent-terminal --project "$PROJECT" send prompt --no-submit -- 'partial input'
```

### press

Press canonical named keys. Keys are space-separated after `--`.

```bash
agent-terminal --project "$PROJECT" press debugger -- Down Enter
agent-terminal --project "$PROJECT" press process -- Ctrl+C
```

### stop

Gracefully stop and clean up a job. Use `--force` after a graceful stop reports `job_still_running`.

```bash
agent-terminal --project "$PROJECT" stop dev-server
agent-terminal --project "$PROJECT" stop dev-server --force
```

### list

List all project-scoped jobs. Use after context loss, cancellation, or when ownership is uncertain.

```bash
agent-terminal --project "$PROJECT" list
```

## Job names and keys

Job names must match `[a-z0-9][a-z0-9._-]{0,63}`. Prefer descriptive lowercase names like
`dev-server`, `python-repl`, or `test-watch`. Avoid names that contain shell-special characters.

Accepted key names: `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert`, `Home`, `End`,
`PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1` through `F12`, `Ctrl+<ASCII letter>`
(e.g. `Ctrl+C`, `Ctrl+D`), and `Alt+<single printable ASCII character>`. Anything else is
rejected by the CLI.

## Interpreting output

Every response is a single compact JSON object on stdout. Diagnostics go to stderr.

### Envelope

```json
{"status":"ok","data":{...}}
{"status":"error","error":{"code":"job_not_found","message":"job \"missing\" was not found","hint":"Run list to see known jobs."}}
```

On success, `status` is `"ok"` and the `data` field contains the command-specific payload.
On error, `status` is `"error"` and the `error` object provides `code`, `message`, and an
optional `hint`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Successful operation, including an empty list |
| 1 | Expected job or lock error (`job_*`, `lock_busy`): a valid request the agent can recover from |
| 2 | Invalid input, controller, backend, or state failure (`invalid_input`, `state_*`, `zellij_*`) |

### Lifecycle states

| State | Meaning |
|---|---|
| `running` | The job accepts input |
| `exited` | The process finished; `exit_code` is present; the screen remains readable until stopped |
| `lost` | Persisted ownership no longer maps to a live terminal; stop it to clean up |

### Recovery codes

- `job_exists`: read or stop the existing job instead of starting another.
- `job_not_found`: run `list`; the name or project scope may be wrong.
- `job_not_running`: read the job before sending more input.
- `job_still_running`: retry `stop` with `--force` to close the pane.
- `lock_busy`: do useful work and retry once later; do not spin.

### Screen field

The `screen` field on `read` is a bounded visible terminal snapshot (newest 200 lines, 32 KiB),
not a complete stdout/stderr log. The `truncated` flag is set when either limit was applied.
The screen is ANSI-stripped rendered PTY content.

## Lifecycle rules

1. Start a job once with `start`. A duplicate start returns `job_exists`.
2. Call `read` before any `send` or `press`. Only a `running` job accepts input.
3. State, not screen activity, determines completion. An unchanged screen never implies the
   job is finished.
4. Output is bounded. For complete logs, redirect output inside the job command rather than
   relying on the screen.
5. Use `stop` for cleanup. A successful `stop` response is authoritative confirmation. Use
   `list` only after context loss, cancellation, or when ownership is uncertain.
6. After an `exited` or `lost` job, `stop` it to free resources.

## Persistent vs foreground

Start a terminal job when at least one holds:

- a server, watcher, or build must keep running while you do other work;
- the process asks questions or needs text/key input;
- you need to return to a REPL, debugger, or interactive program;
- a human may need to inspect the same terminal job.

Do not create a terminal job for a quick command that can finish in one Bash call.

## Safety

- Never put secrets in job names, commands, or sent text.
- Use `stop` for lifecycle cleanup, not a raw `Ctrl+C` keypress.
- Read before send or press. Do not send input to a job whose state you have not confirmed.
- Do not reason about Zellij sessions, panes, or numeric IDs. The job name is the only public
  handle.
- Do not claim every descendant OS process was killed. Forced stop guarantees terminal closure
  and controller-state cleanup.

## Shell quoting

Quote variables with double quotes: `"$PROJECT"`, `"$CWD"`. Quote literal text with single
quotes: `'hello world'`. For text containing an apostrophe, close the single-quoted segment
and append an escaped apostrophe: `'don'"'"'t'`.

## Cancellation

When a Bash invocation is cancelled mid-execution:

- Cancelled `start` or `stop` must be reconciled by a same-scope `list` or `read`. A
  cancelled `start` may still have launched the job; a cancelled `stop` may not have cleaned
  it up.
- Never automatically replay cancelled `send` or `press`. That the command was issued does
  not prove the terminal consumed the input.
- `read` and `list` are safe to retry after cancellation.

## Permissions

This skill adds no dedicated `terminal_*` permissions. OpenCode's Bash authorization governs
every invocation of `agent-terminal` because the skill runs the CLI as a standard Bash command.
Structured external `workdir` and recognized filesystem commands may receive additional checks,
but the embedded `--project` and `--cwd` arguments are not independently canonicalized or
authorized by OpenCode. Bash remains unsandboxed.

## Migration from the old adapter

This skill replaces the TypeScript custom-tool adapter. Key differences:

- Invoke the CLI through Bash instead of calling the old TypeScript tool functions.
- There are no custom tool-call forms. Every operation is a Bash command.
- `--project` must be explicit on every call; the CLI no longer receives project scope from
  the host.
- `$PROJECT` and `$CWD` do not persist across Bash calls.
- Job names, key names, lifecycle states, and error codes are unchanged.
