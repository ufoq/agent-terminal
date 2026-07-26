---
name: agent-terminal
description: "Run persistent or interactive terminal jobs through a simple Zellij wrapper. Use for servers, watchers, REPLs, debuggers, and anything that must survive across turns. Prefer plain Bash for short foreground commands."
compatibility: Requires the agent-terminal binary on PATH and a Bash tool.
metadata:
  project: agent-terminal
  version: "1"
---

# Agent Terminal

`agent-terminal` gives you persistent, project-scoped terminal jobs with a small, stable CLI. Use it when the process must outlive one Bash call or needs interactive input. Use plain Bash for anything that finishes in one call.

The CLI finds your project root by walking up to the nearest `.git` directory; it falls back to the current directory. Jobs are identified by stable names, not Zellij sessions or pane IDs.

## Commands

Start a job. Everything after `--` is passed as the command argv.

```bash
agent-terminal start dev-server -- npm run dev
agent-terminal start build --cwd ./packages/api -- npm run build
agent-terminal start build -- /bin/sh -c 'make -j"$(nproc)" 2>&1 | tee build.log'
```

Read state and the bounded visible screen.

```bash
agent-terminal read dev-server
```

Send text. Enter is appended by default; use `--no-submit` to avoid it.

```bash
agent-terminal send repl -- '2 + 2'
agent-terminal send prompt --no-submit -- 'partial input'
```

Press named keys.

```bash
agent-terminal press debugger -- Down Enter
agent-terminal press process -- Ctrl+C
```

Stop a job. Use `--force` only if a graceful stop reports `job_still_running`.

```bash
agent-terminal stop dev-server
agent-terminal stop dev-server --force
```

List jobs in the current project scope.

```bash
agent-terminal list
```

## Optional overrides

- `--project <PATH>` — force a project scope other than the nearest Git root.
- `--cwd <PATH>` — run the job in a directory other than the invocation directory.
- `--state-dir <PATH>` — override where state is stored (default: OS state directory).

## Job names and keys

Job names must match `[a-z0-9][a-z0-9._-]{0,63}`. Use descriptive lowercase names like `dev-server`, `python-repl`, or `test-watch`.

Key names: `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1` through `F12`, `Ctrl+<ASCII letter>`, and `Alt+<single printable ASCII character>`.

## Output

Every response is JSON. Success:

```json
{"status":"ok","data":{...}}
```

Error:

```json
{"status":"error","error":{"code":"job_not_found","message":"job \"missing\" was not found","hint":"Run list to see known jobs."}}
```

The JSON `error.code` and `hint` are authoritative. Nonzero exit codes only mean recovery is needed, not a crash.

## Lifecycle

| State | Meaning |
|---|---|
| `running` | The job accepts input. |
| `exited` | The process finished; `exit_code` is present; the screen remains readable until stopped. |
| `lost` | Persisted ownership no longer maps to a live terminal; stop it to clean up. |

Common recovery codes:

- `job_exists` — read or stop the existing job instead of starting another.
- `job_not_found` — run `list` and check the project scope.
- `job_still_running` — retry `stop` with `--force` if forced closure is acceptable.

The `screen` field on `read` is a bounded visible snapshot (newest 200 lines, 32 KiB), not a complete log. For complete logs, redirect output inside the job command.

## Rules

1. Use `start` once per job name. Duplicate starts return `job_exists`.
2. `read` before sending input when you do not already know the prompt or state.
3. State, not screen activity, determines completion. An unchanged screen does not mean the job is finished.
4. Use `stop` for cleanup. A successful `stop` is authoritative.
5. Use `list` only after context loss, cancellation, or when ownership is uncertain.
6. After `exited` or `lost`, call `stop` to free resources.

## Cancellation

If a Bash call is cancelled mid-operation, reconcile with `list` or `read` before continuing. Do not automatically replay cancelled `send` or `press`; being issued does not prove the terminal consumed the input. `read` and `list` are safe to retry.

## Why not raw Zellij?

Raw `zellij` commands expose sessions, pane IDs, and screen text. An agent using them directly would have to track pane ownership, parse unstructured output, handle concurrent calls safely, and recover from its own crashes. `agent-terminal` hides that behind logical job names, a JSON envelope, bounded screen output, atomic state, and ownership reconciliation.
