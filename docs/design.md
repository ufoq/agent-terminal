# agent-terminal design

Status: implemented and integration-tested with real OpenCode.

## 1. Product rule

The agent understands **jobs**, not Zellij.

A job is a named, project-scoped terminal process that can outlive one tool call, expose its current screen and exit state, accept later input, and be cleaned up explicitly. Sessions, tabs, panes, focus, sockets, and Zellij IDs are private implementation details.

The interface is optimized for coding agents first and direct CLI users second.

## 2. Why the previous surface was rejected

The previous `terminal_job({ action, ...optionalFields })` shape made the model solve validation rules that belong in schemas. `start`, `read`, `send`, `stop`, and `list` have different required arguments, safety properties, and permissions.

OpenCode natively turns named exports into separate tools. Existing coding agents likewise separate background start, output, and stop operations. Narrow tools therefore provide:

- one purpose and one short description per tool;
- no irrelevant or conditionally required arguments;
- independent permission policy for read-only and destructive operations;
- simpler error recovery;
- no top-level action enum.

The design also rejects a traditional process-manager surface. Separate `status`, `output`, `wait`, and `cleanup` commands would be flexible, but they make the agent orchestrate the controller instead of doing its real task.

## 3. Model-facing OpenCode tools

One file, `opencode/tools/terminal.ts`, exports six tools. OpenCode exposes them as `terminal_start`, `terminal_read`, `terminal_send`, `terminal_press`, `terminal_stop`, and `terminal_list`.

### `terminal_start`

Start a persistent or interactive terminal job.

```text
terminal_start(job, command, cwd?)
```

| Argument | Type | Meaning |
|---|---|---|
| `job` | string | Agent-chosen stable name such as `dev-server` or `debugger` |
| `command` | string | Shell command to run |
| `cwd` | string, optional | Working directory; defaults to OpenCode `context.directory` |

Use for dev servers, watchers, REPLs, debuggers, long builds, and commands that may need later input. Do not use for short one-shot commands; use the normal shell tool.

The adapter supplies the project root from `context.worktree`; if OpenCode reports the filesystem root for a non-Git directory, it uses `context.directory`. The model never supplies project identity.

### `terminal_read`

Read one job's lifecycle state and current terminal screen.

```text
terminal_read(job)
```

This deliberately combines status and output. In the common workflow the agent needs both, and one combined read avoids a second tool call. The state field, not screen activity, determines whether the job has exited.

### `terminal_send`

Send literal text to a running job.

```text
terminal_send(job, text, submit=true)
```

`submit=true` presses Enter after the text. Set it to false for raw paste. The text is passed as data, never interpolated into a controller shell command.

### `terminal_press`

Send named keys to a running job.

```text
terminal_press(job, keys)
```

`keys` is a non-empty array such as `["Down", "Enter"]`, `["Ctrl+D"]`, or `["F5"]`. Use `terminal_stop`, not `terminal_press(..., ["Ctrl+C"])`, when the intent is to terminate a job.

Text and keys are separate tools because they have different semantics and because a single schema with optional `text` and `keys` permits invalid calls.

### `terminal_stop`

Stop or clean up one known job.

```text
terminal_stop(job, force=false)
```

Without `force`, send the terminal's Ctrl+C key sequence and allow five seconds for observed exit. If it remains running, return `job_still_running` and leave it intact. With `force`, close the owned terminal pane immediately.

The operation returns the last screen observed before closure when available. It promises only that the controller-owned pane is closed and its registry entry removed, not that the application received a signal or that detached descendants were killed.

### `terminal_list`

List all jobs in the current project.

```text
terminal_list()
```

Use after context loss or when the agent no longer remembers the job name. It returns concise summaries and never returns screen content.

## 4. Why there are exactly six tools

| Omitted operation | Reason |
|---|---|
| `status` | `terminal_read` already returns authoritative state with the screen |
| `output` / `tail` | `terminal_read` provides the bounded screen that matters for PTY interaction |
| `wait` | A blocking wait prevents the agent from doing useful work while the job runs |
| `cleanup` | `terminal_stop` closes and forgets exited or lost jobs |
| `interrupt` | Ctrl+C dispatch is the default `terminal_stop` behavior |
| `attach` | Human visibility is a backend property, not an agent operation |
| raw Zellij action | It would break ownership and force the agent to reason about panes |

No persistent output log or completion notification is included. Those require a runner or service layer and are not needed for the minimal PTY workflow.

## 5. Standalone CLI

The Rust binary exposes the same six operations as subcommands:

```text
agent-terminal [GLOBAL OPTIONS] start <JOB> [--cwd <PATH>] -- <PROGRAM> [ARG...]
agent-terminal [GLOBAL OPTIONS] read <JOB>
agent-terminal [GLOBAL OPTIONS] send <JOB> [--no-submit] -- <TEXT>
agent-terminal [GLOBAL OPTIONS] press <JOB> -- <KEY>...
agent-terminal [GLOBAL OPTIONS] stop <JOB> [--force]
agent-terminal [GLOBAL OPTIONS] list
```

Global options:

```text
--project <PATH>   Project identity and state scope; defaults to current directory
--pretty           Pretty-print JSON; compact JSON is the default
-v, -vv, -vvv      Stderr diagnostic level
--state-dir <PATH> Override state root for testing or isolated use
```

The core CLI takes an argv after `--`; it never parses a shell string. The OpenCode adapter accepts a command string for model ergonomics. It uses `SHELL` only when it names an absolute executable, otherwise falls back to `/bin/sh`, and invokes non-login `-c` mode:

```text
agent-terminal --project <scope> start <job> --cwd <cwd> -- <shell> -c <command>
```

The command remains one argv element. The adapter does not concatenate or re-quote it. The CLI and OpenCode tool both submit Enter after `send` by default; `--no-submit` maps to `submit=false`.

Exit codes:

| Code | Meaning |
|---|---|
| `0` | Successful operation, including an empty list |
| `1` | `job_*` or `lock_busy`: a valid request that the agent can recover from |
| `2` | `invalid_input`, `state_*`, or `zellij_*`: request/controller/backend failure |

## 6. JSON contract

Stdout contains exactly one JSON object plus a newline. Diagnostics go only to stderr. Argument parsing and validation use the same error envelope; `--help` and `--version` are the only plain-text exceptions.

Success:

```json
{"status":"ok","data":{"job":"dev-server","state":"running"}}
```

Error:

```json
{"status":"error","error":{"code":"job_not_found","message":"No job named 'api'.","hint":"Run terminal_list to see known jobs."}}
```

The TypeScript adapter parses stdout on every exit code. A valid error envelope is returned to the model as a normal structured tool result so the model can recover. It throws only when the binary produced no valid envelope.

### Operation data

`start` returns reconciled state, never an assumed state:

```json
{"job":"dev-server","state":"running"}
```

```json
{"job":"fast-check","state":"exited","exit_code":7}
```

```json
{"job":"vanished-during-start","state":"lost"}
```

`read` while running:

```json
{"job":"dev-server","state":"running","screen_available":true,"screen":"ready on :3000\n","truncated":false}
```

`read` after exit:

```json
{"job":"tests","state":"exited","exit_code":1,"screen_available":true,"screen":"2 tests failed\n","truncated":false}
```

`read` for a lost job has no screen:

```json
{"job":"server","state":"lost","screen_available":false}
```

Successful text and key dispatch mean only that input was issued after an immediately preceding ownership/running check:

```json
{"job":"repl","issued":"text","submitted":true}
```

```json
{"job":"debugger","issued":"keys","keys":["Down","Enter"]}
```

`stop` reports cleanup and the last pre-close screen when capture succeeded:

```json
{"job":"dev-server","cleaned_up":true,"forced":false,"screen_available":true,"last_screen":"shutting down\n","truncated":false}
```

If capture fails during cleanup, `stop` can still succeed with `screen_available:false`; it never silently omits an ambiguously optional screen. `forced=true` means the running-force closure branch was actually used, not merely requested; cleanup of an already exited or lost job returns `forced=false`. `list` returns:

```json
{"jobs":[{"job":"dev-server","state":"running"},{"job":"tests","state":"exited","exit_code":1}]}
```

Optional-field rules are fixed: `exit_code` appears only for `exited`; `screen` and `truncated` appear on successful `read` only when `screen_available=true`; `last_screen` and `truncated` appear on `stop` only when `screen_available=true`.

## 7. Job semantics

### Identity

- `job` is the only public handle.
- Names match `[a-z0-9][a-z0-9._-]{0,63}`.
- Names are unique within one canonical project root.
- A duplicate start returns `job_exists`, even if the prior job exited. The agent must read or stop the existing job first.
- Pane and session IDs never appear in normal responses.

### States

Only three persisted/live states are exposed:

| State | Meaning |
|---|---|
| `running` | The owned terminal command has not exited |
| `exited` | The command exited; `exit_code` is present and the pane remains held for capture |
| `lost` | State records the job, but its owned pane or session disappeared |

There is no public `starting` state because `start` returns only after the pane ID is acquired, state is durable, and live state is reconciled. A fast command may therefore make `start` return `exited` with `exit_code`. There is no separate `failed` state; failure is `exited` with a non-zero `exit_code`. Stopped jobs are removed rather than retained as tombstones.

Job panes are always launched in held-on-exit mode by omitting Zellij's `--close-on-exit`; preserving fast exit state and screen is an invariant, not a timing assumption.

### Read

- Reconcile registry state with `list-panes --json` first.
- Capture plain-text screen and scrollback through a unique, initially absent dump path.
- Require the current invocation to create the dump file; otherwise return `zellij_failed` rather than empty or stale output.
- Return the newest at most 200 lines and 32 KiB.
- Set `truncated=true` when either limit was applied.
- The screen is rendered PTY content, not a canonical stdout/stderr log.
- An unchanged screen never implies completion.

### Input

- Accept input only for a known `running` job.
- Validate session ownership plus terminal ID, non-plugin type, and the nonce-bearing pane title immediately before sending because Zellij silently succeeds for unknown pane IDs and plugin/terminal numeric IDs can collide.
- Serialize state lookup and input under the per-project lock.
- Use bracketed paste for text and `send-keys` for named keys.
- Success means input was issued after validation, not that the application consumed it.

Public key names use one controller-owned grammar, independent of Zellij spelling: `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1` through `F12`, `Ctrl+<ASCII letter>`, and `Alt+<ASCII character>`. The controller validates and translates these names.

### Stop

- `running`, graceful: send the Ctrl+C terminal key sequence, poll authoritative pane state for up to five seconds, then capture and close only after observed exit.
- `running`, force: capture what is available as `last_screen`, close the owned pane, and remove the job record after the pane is absent.
- `exited`: capture the last screen, close the held pane, and remove the record.
- `lost`: remove stale state and report successful cleanup.
- unknown name: return `job_not_found`; never silently accept a typo.

The post-close absence check is required because Zellij reports success for an unknown pane ID. It is not generic defensive verification. Graceful success proves observed exit; forced success proves only pane absence and registry cleanup.

## 8. Error codes

Public stable codes:

| Code | Agent response |
|---|---|
| `job_exists` | Read or stop the existing job; do not start a duplicate |
| `job_not_found` | List jobs and correct the name |
| `job_not_running` | Read the exited/lost job; do not send input |
| `job_still_running` | Retry stop with `force=true` only if forced closure is acceptable |
| `invalid_input` | Correct the named argument |
| `lock_busy` | Retry the same operation |
| `zellij_not_found` | Install Zellij 0.44.3 or later |
| `zellij_failed` | Preserve concise backend context in `message` |
| `state_io` | Report the state path in error context |
| `state_corrupt` | Do not overwrite unreadable state automatically |

Error messages are concise and actionable. Rust backtraces, command dumps, pane IDs, and raw Zellij JSON never enter the model response.

## 9. Ownership and persistence

One controller-owned Zellij session exists per canonical project root. Its stable name combines a project digest with a persisted random ownership nonce. Jobs are created as held-on-exit panes and titled `agent-terminal:<job>:<operation-nonce>`.

State lives under the platform state directory:

```text
<state-root>/
├── bootstrap.lock
└── projects/<project-digest>/
    ├── state.json
    └── state.lock
```

The permanent per-project sibling lock is held across each complete read-modify-Zellij-write operation. Its acquisition is non-blocking: contention returns `lock_busy` immediately. A state-root bootstrap lock serializes only Zellij session creation/readiness across projects, avoiding backend startup contention while keeping later job operations independent. Waiting for that lock is included in the same ten-second bootstrap deadline. The empty `bootstrap.lock` file intentionally persists after jobs exit so later processes reuse the same synchronization inode; it contains no job data. Every individual Zellij invocation has a two-second timeout. Interrupted `pending_start` adoption also has a ten-second overall deadline; expiry returns `zellij_failed` while leaving `pending_start` durable for later reconciliation or `stop`. Graceful stop likewise has a ten-second overall deadline including its five-second exit wait, so it monopolizes the project lock only for a bounded interval.

`state.json` is replaced atomically through a same-directory temporary file. It stores project root, ownership nonce, session name, job name, operation nonce, full pane identity, cwd, argv, creation time, and internal mutation phase. Runtime state is derived from Zellij.

State mutations use two internal phases that are never exposed as job states:

- `pending_start` is durable before session/pane creation. Reconciliation adopts only a non-plugin pane whose session, terminal ID, and nonce-bearing title all match. Authoritative disappearance after a pane identity was acquired becomes `lost`. If no pane identity appears by the adoption deadline, return `zellij_failed` and retain `pending_start` for later reconciliation or `stop`.
- `pending_remove` is durable before pane closure. Reconciliation retries closure when the owned pane remains. For the last job, the phase stays durable until both the job pane and the owned keeper-only session are absent; session-deletion failure returns `zellij_failed` and preserves the phase. Only then does reconciliation remove the job entry and clear the phase.

This ordering keeps a pane created during an interrupted controller process or cancelled OpenCode call reconcilable by its durable nonce. State is written before the session exists, so missing or corrupt state never authorizes targeting a pre-existing session.

The controller supplies a minimal private Zellij config and layout so headless startup does not depend on user configuration. It bootstraps with `attach --create-background` and a keeper pane titled with the ownership nonce, then polls within the ten-second startup deadline until that pane is visible before creating a job. Plugin panes are always ignored because Zellij can retain a hidden plugin with the same numeric ID as a terminal pane. Last-job cleanup deletes the owned session inside `pending_remove` before state is cleared.

## 10. Internal architecture derived from the interface

The standalone project is one Rust crate with a small library and binary:

```text
src/
├── main.rs        parse, tracing, one response, exit code
├── cli.rs         six subcommands and global options
├── error.rs       typed domain errors and stable codes
├── output.rs      JSON envelopes; sole stdout writer
├── paths.rs       project/config/state path resolution
├── state.rs       typed registry, lock, atomic persistence
├── zellij.rs      subprocess adapter behind a testable trait
└── commands/      start, read, send, press, stop, list
```

Rules:

- synchronous `std::process::Command`; no async runtime or daemon;
- argv slices only; no formatted shell commands in Rust;
- `thiserror` in the library and `anyhow` only in `main` for dynamic context;
- no `unwrap`, `expect`, `panic`, `Box<dyn Error>`, or stdout writes in library code;
- newtypes for job, project, session, and terminal-pane identity;
- compact serde JSON by default, tracing to stderr;
- Zellij 0.44.3 is the minimum tested version.

The TypeScript adapter contains only schemas, context-to-CLI argument mapping, cancellation-aware process spawning, and JSON result translation. It contains no job lifecycle logic.

## 11. Verification contract for implementation

1. Start/read/stop: start a dev server, read `running` plus its ready screen, stop it, and prove the pane/session is cleaned up.
2. Interactive text: start a prompt or REPL, send literal text with submission, and read the resulting screen.
3. Named keys: start an interactive program, press named navigation keys, and read the changed screen.
4. Fast failure: start a command that exits non-zero immediately and read `exited`, exact `exit_code`, and last screen.
5. Recovery: distinguish duplicate name, unknown name, exited job, and externally lost pane.
6. Isolation: run same-named jobs in two project roots without state/session collision.
7. Crash recovery: terminate the controller between durable pending state and each Zellij mutation, then prove the next operation adopts or removes exactly the nonce-matched pane.
8. OpenCode surface: load six named exports, cancel a running adapter call through `context.abort`, and verify no action-enum tool exists.

Real-Zellij tests use isolated HOME/XDG directories and the installed Zellij binary. Unit tests use a fake backend only for deterministic domain/error paths.

## 12. Explicit non-goals

- No runner script, persistent output log, event subscription, or notification service.
- No `wait`, retry, restart, pause, resume, attach, cleanup, or raw Zellij command.
- No daemon, HTTP, socket, MCP server, Zellij WASM plugin, or database.
- No arbitrary user-pane control or shared-session ownership.
- No Pi adapter yet.
- No automatic secret redaction; commands and screen output are inherently visible in the terminal.

## 13. Project and distribution

- Standalone Rust project, independent of `myb` conventions.
- MIT license.
- Normal Cargo workflow: `cargo build --release` and `cargo install --path .`.
- Release archives may be added later; they are not part of the initial implementation.

## 14. Evidence used

- OpenCode custom tools and named exports: <https://opencode.ai/docs/custom-tools/>
- OpenAI function-tool schema guidance: <https://platform.openai.com/docs/guides/function-calling>
- Anthropic agent-tool design principles: <https://www.anthropic.com/engineering/writing-tools-for-agents>
- Zellij programmatic control: <https://zellij.dev/documentation/programmatic-control.html>
- Zellij CLI actions: <https://zellij.dev/documentation/cli-actions.html>
- Prior local synthesis: `../../../zellij-work/05-zellij-for-agents-analysis.md`

The main borrowed pattern is Claude Code's split between background start, output/state read, and stop. The design adds only the input and recovery operations required for persistent interactive PTY work.
