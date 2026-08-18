# agent-terminal design

Status: Rust CLI implemented; OpenCode skill is CLI-oriented (invokes the binary through Bash).

## 1. Product rule

The agent understands **jobs**, not Zellij.

A job is a named, project-scoped terminal process that can outlive one tool call, expose its current screen and exit state, accept later input, and be cleaned up explicitly. Sessions, tabs, panes, focus, sockets, and Zellij IDs are private implementation details.

The wrapper is worthwhile because raw `zellij` commands force the agent to track pane ownership, parse unstructured output, handle concurrent calls safely, and recover from its own crashes. `agent-terminal` hides that behind logical job names, a JSON envelope, bounded screen output, atomic state, and ownership reconciliation.

The interface is optimized for coding agents first and direct CLI users second.

## 2. Standalone CLI

The Rust binary exposes the same six operations as subcommands:

```text
agent-terminal [GLOBAL OPTIONS] start <JOB> [--cwd <PATH>] -- <PROGRAM> [ARG...]
agent-terminal [GLOBAL OPTIONS] read <JOB>
agent-terminal [GLOBAL OPTIONS] send <JOB> [--no-submit] -- <TEXT>
agent-terminal [GLOBAL OPTIONS] press <JOB> -- <KEY>...
agent-terminal [GLOBAL OPTIONS] stop <JOB>
agent-terminal [GLOBAL OPTIONS] list
```

Global options:

```text
--project <PATH>   Project identity and state scope; defaults to the nearest Git root,
                   or the current directory when not inside a Git repository
--pretty           Pretty-print JSON; compact JSON is the default
-v, -vv, -vvv      Stderr diagnostic level
--state-dir <PATH> Override state root for testing or isolated use
```

`start` additionally accepts `--cwd <PATH>`; it defaults to the invocation directory.

The core CLI takes an argv after `--`; it never parses a shell string. Defaulting `--project` to the Git root and `start --cwd` to the invocation directory removes the need for the agent to derive and pass these values on every call.

Exit codes:

| Code | Meaning |
|---|---|
| `0` | Successful operation, including an empty list |
| `1` | `job_exists`, `job_not_found`, `job_not_running`, `delivery_uncertain`, or `lock_busy`: a valid request that the agent can recover from |
| `2` | `invalid_input`, `zellij_not_found`, `zellij_failed`, `state_io`, or `state_corrupt`: request/controller/backend failure |

## 3. JSON contract

Stdout contains exactly one JSON object plus a newline. Diagnostics go only to stderr. Argument parsing and validation use the same error envelope; `--help` and `--version` are the only plain-text exceptions.

Success:

```json
{"status":"ok","state":"running"}
```

Error:

```json
{"status":"error","code":"job_not_found","message":"No job named 'api'."}
```

### Operation data

`start` returns reconciled state, never an assumed state:

```json
{"status":"ok","state":"running"}
```

```json
{"status":"ok","state":"exited","exit_code":7}
```

`read` while running:

```json
{"status":"ok","state":"running","screen":"ready on :3000\n","truncated":false}
```

`read` after exit:

```json
{"status":"ok","state":"exited","exit_code":1,"screen":"2 tests failed\n","truncated":false}
```

Successful text and key dispatch mean only that input was issued after an immediately preceding ownership/running check:

```json
{"status":"ok"}
```

`stop` returns a unit acknowledgement:

```json
{"status":"ok"}
```

`list` returns:

```json
{"status":"ok","jobs":[{"job":"dev-server","state":"running"},{"job":"tests","state":"exited","exit_code":1}]}
```

Optional-field rules are fixed: `exit_code` appears only for `exited`; `screen` is always present on successful `read`; `truncated` indicates whether output exceeded the 200-line or 32 KiB bound.

## 4. Job semantics

### Identity

- `job` is the only public handle.
- Names match `[a-z0-9][a-z0-9._-]{0,63}`.
- Names are unique within one canonical project root.
- A duplicate start returns `job_exists`, even if the prior job exited. The agent must read or stop the existing job first.
- Pane and session IDs never appear in normal responses.

### States

Only two persisted/live states are exposed:

| State | Meaning |
|---|---|
| `running` | The owned terminal command has not exited |
| `exited` | The command exited; `exit_code` is present and the pane remains held for capture |

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
- Success means input was issued after validation, not that the application consumed it. If the job disappears between validation and delivery, the CLI returns `delivery_uncertain`; the agent must read the job before deciding whether to resend.

Public key names use one controller-owned grammar, independent of Zellij spelling: `Enter`, `Tab`, `Esc`, `Backspace`, `Delete`, `Insert`, `Home`, `End`, `PageUp`, `PageDown`, `Up`, `Down`, `Left`, `Right`, `F1` through `F12`, `Ctrl+<ASCII letter>`, and `Alt+<ASCII character>`. The controller validates and translates these names.

### Stop

- Stop sends Ctrl+C, waits up to 5 seconds for the command to exit, then force-closes the pane and removes the job record after the pane is absent. It auto-escalates; the caller does not select a mode.
- `exited`: close the held pane and remove the record.
- stale registry entry (pane already gone): reconcile internally to `job_not_found`.
- unknown name: return `job_not_found`; never silently accept a typo.

The post-close absence check is required because Zellij reports success for an unknown pane ID. It is not generic defensive verification. Stop auto-escalates: it sends Ctrl+C, waits up to 5 seconds for graceful exit, then force-closes the pane and removes the job record after confirming pane absence.

## 5. Error codes

Public stable codes:

| Code | Agent response |
|---|---|
| `job_exists` | Read or stop the existing job; do not start a duplicate |
| `job_not_found` | List jobs and correct the name |
| `job_not_running` | Read the exited job; do not send input |
| `delivery_uncertain` | Read the job before deciding whether to resend (send/press only) |
| `invalid_input` | Correct the named argument |
| `lock_busy` | Retry the same operation |
| `zellij_not_found` | Install Zellij 0.44.3 or later |
| `zellij_failed` | Report the terminal backend operation failed; detailed backend context stays in logs, never in the public message |
| `state_io` | Report the state operation failed; the filesystem path (which embeds the scope digest) stays in logs, never in the public message |
| `state_corrupt` | Do not overwrite unreadable state automatically; do not publish the corrupt file's path |

Error messages are concise and actionable. Rust backtraces, command dumps, pane IDs, and raw Zellij JSON never enter the model response.

## 6. Ownership and persistence

One controller-owned Zellij session exists per canonical project root. Its stable name combines a project digest with a persisted random ownership nonce. Jobs are created as held-on-exit panes and titled `agent-terminal:<job>:<operation-nonce>`.

State lives under the platform state directory:

```text
<state-root>/
└── scopes/<scope-digest>/
    ├── bootstrap.lock
    └── projects/<project-digest>/
        ├── state.json
        └── state.lock
```

The permanent per-project sibling lock is held across each complete read-modify-Zellij-write operation. Its acquisition is non-blocking: contention returns `lock_busy` immediately. A per-scope bootstrap lock serializes only Zellij session creation/readiness within that scope, avoiding backend startup contention while keeping later job operations independent. Waiting for that lock is included in the same ten-second bootstrap deadline. The empty `bootstrap.lock` file intentionally persists after jobs exit so later processes reuse the same synchronization inode; it contains no job data. Every individual Zellij invocation has a two-second timeout. Interrupted `pending_start` adoption also has a ten-second overall deadline; expiry returns `zellij_failed` while leaving `pending_start` durable for later reconciliation or `stop`. Graceful stop likewise has a fifteen-second overall deadline including its five-second exit wait, so it monopolizes the project lock only for a bounded interval.

`state.json` is replaced atomically through a same-directory temporary file. It stores project root, ownership nonce, session name, job name, operation nonce, full pane identity, cwd, argv, creation time, and internal mutation phase. Runtime state is derived from Zellij.

State mutations use two internal phases that are never exposed as job states:

- `pending_start` is durable before session/pane creation. Reconciliation adopts only a non-plugin pane whose session, terminal ID, and nonce-bearing title all match. Authoritative disappearance after a pane identity was acquired cleans the record and surfaces `job_not_found`. If no pane identity appears by the adoption deadline, return `zellij_failed` and retain `pending_start` for later reconciliation or `stop`.
- `pending_remove` is durable before pane closure. Reconciliation retries closure when the owned pane remains. For the last job, the phase stays durable until both the job pane and the owned keeper-only session are absent; session-deletion failure returns `zellij_failed` and preserves the phase. Only then does reconciliation remove the job entry and clear the phase.

This ordering keeps a pane created during an interrupted controller process or cancelled OpenCode call reconcilable by its durable nonce. State is written before the session exists, so missing or corrupt state never authorizes targeting a pre-existing session.

The controller supplies a minimal private Zellij config and layout so headless startup does not depend on user configuration. It bootstraps with `attach --create-background` and a keeper pane titled with the ownership nonce, then polls within the ten-second startup deadline until that pane is visible before creating a job. Plugin panes are always ignored because Zellij can retain a hidden plugin with the same numeric ID as a terminal pane. Last-job cleanup deletes the owned session inside `pending_remove` before state is cleared.

## 7. Internal architecture derived from the interface

The standalone project is one Rust crate with a small library and binary:

```text
src/
├── main.rs        parse, tracing, one response, exit code
├── cli.rs         six subcommands and global options
├── error.rs       typed domain errors and stable codes
├── output.rs      JSON envelopes; sole stdout writer
├── paths.rs       project/config/state path resolution; Git-root default
├── domain.rs      job/session/pane newtypes, key grammar, bounded screen
├── config.rs      private Zellij config/layout file materialization
├── controller.rs  job lifecycle orchestration behind a testable trait
├── reconcile.rs   pending-start and pending-remove reconciliation
├── telemetry.rs   tracing initialization
├── state.rs       re-exports from the state module tree
├── state/         typed registry, lock, atomic persistence
├── zellij.rs      re-exports from the zellij module tree
├── zellij/        subprocess backend behind a testable trait
└── commands/      start, read, send, press, stop, list
```

Rules:

- synchronous `std::process::Command`; no async runtime or daemon;
- argv slices only; no formatted shell commands in Rust;
- `thiserror` for typed errors throughout the library and binary; no `anyhow`;
- no `unwrap`, `expect`, `panic`, `Box<dyn Error>`, or stdout writes in library code;
- newtypes for job, project, session, and terminal-pane identity;
- compact serde JSON by default, tracing to stderr;
- Zellij 0.44.3 is the minimum tested version.

## 8. Verification contract for implementation

1. Start/read/stop: start a dev server, read `running` plus its ready screen, stop it, and prove the pane/session is cleaned up.
2. Interactive text: start a prompt or REPL, send literal text with submission, and read the resulting screen.
3. Named keys: start an interactive program, press named navigation keys, and read the changed screen.
4. Fast failure: start a command that exits non-zero immediately and read `exited`, exact `exit_code`, and last screen.
5. Recovery: distinguish duplicate name, unknown name, exited job, and an externally closed pane (reconciles to `job_not_found`).
6. Isolation: run same-named jobs in two project roots without state/session collision.
7. Crash recovery: terminate the controller between durable pending state and each Zellij mutation, then prove the next operation adopts or removes exactly the nonce-matched pane.

Real-Zellij tests use isolated HOME/XDG directories and the installed Zellij binary. Unit tests use a fake backend only for deterministic domain/error paths.

## 9. Explicit non-goals

- No runner script, persistent output log, event subscription, or notification service.
- No `wait`, retry, restart, pause, resume, attach, cleanup, or raw Zellij command.
- No daemon, HTTP, socket, MCP server, Zellij WASM plugin, or database.
- No arbitrary user-pane control or shared-session ownership.
- No automatic secret redaction; commands and screen output are inherently visible in the terminal.

## 10. Project and distribution

- Standalone Rust project, independent of `myb` conventions.
- MIT license.
- Normal Cargo workflow: `cargo build --release` and `cargo install --path .`.
- Release archives may be added later; they are not part of the initial implementation.

## 11. Evidence used

- Anthropic agent-tool design principles: <https://www.anthropic.com/engineering/writing-tools-for-agents>
- Zellij programmatic control: <https://zellij.dev/documentation/programmatic-control.html>
- Zellij CLI actions: <https://zellij.dev/documentation/cli-actions.html>

The main borrowed pattern is Claude Code's split between background start, output/state read, and stop. The design adds only the input and recovery operations required for persistent interactive PTY work.

## 12. Cross-agent scoping

The CLI uses an invisible `AGENT_TERMINAL_SCOPE` environment variable to isolate terminal state across concurrent agent sessions. By default the plugin sets it to the OpenCode session id (never the model). The variable is optional and overridable: agents that need to share terminal state — for example a parent and a subagent working on the same task — each set it to the same task-specific value (such as `20260803-fix-auth-refactor`) so they land in one intended scope without coupling to any specific session. The plugin honors an explicit value unchanged and only auto-injects the session id when the variable is unset.

State is stored under a scoped directory tree:

```text
<state-root>/scopes/<scope-digest>/projects/<project-digest>/
```

Each scope also gets a private Zellij socket namespace under `<tmp>/agent-terminal-<scope-digest>`. Concurrent agents running in separate OpenCode sessions never see each other's jobs, even when they operate on the same project root. Session identity, pane ownership, and locks are all contained within one scope.

When `AGENT_TERMINAL_SCOPE` is unset, the CLI falls back to `PI_SESSION_ID` (pi's native per-session identity) and only then to the literal scope `standalone`. `standalone` is the default when running the binary directly outside of an agent session. Note that omp does not expose `PI_SESSION_ID` to bash commands, so under omp the extension is required for per-session isolation; without it, omp sessions share the `standalone` scope.

The model does not manage scoping. It refers to jobs by their names and relies on the plugin to set the scope behind the scenes. The same job name may be used independently in different scopes without collision.

## 13. pi/omp integration

The pi coding agent (and omp, which is pi-based) is supported through a TypeScript extension in `pi/npm/src/index.ts`, built into two npm packages by `pi/npm/scripts/build.mjs`:

- `@ufoq/pi-agent-terminal` — the extension, the static Linux x86_64 `agent-terminal` binary, and the skill. The host must have Zellij on `PATH`.
- `@ufoq/pi-agent-terminal-bundle-zellij` — same, plus a pinned Zellij 0.44.3 binary.

Both packages declare the extension entry point in their `pi.extensions` manifest field. The extension is loaded via pi's `-e` flag or auto-discovered from `~/.pi/agent/extensions/` (pi) or `~/.omp/agent/extensions/` (omp).

### Scope injection

The extension re-registers the bash tool with a `spawnHook` that injects `AGENT_TERMINAL_SCOPE` into every bash command the agent runs, mirroring the OpenCode plugin's `shell.env` hook. The session id is derived in this order:

1. `AGENT_TERMINAL_SCOPE` — explicit, overridable, honored unchanged.
2. `PI_SESSION_ID` — pi's native per-session environment variable.
3. `PI_SESSION_FILE` — omp's session transcript path; the session id is extracted from the filename (the segment after the last underscore, minus the `.jsonl` extension).
4. `ctx.sessionManager.getSessionId()` — captured from the execute context. omp does not inject session environment variables into the bash tool env (even with `exposeSessionEnvironment`), so the execute wrapper reads the session id from the runtime context and stores it for the `spawnHook` to use as a fallback.

The `spawnHook` also prepends the bundled binary directories (`bin/linux-x64` and, for the bundle variant, `bin/zellij`) to `PATH`, so `agent-terminal` and Zellij resolve inside bash commands without host installs. The extension additionally mutates `process.env.PATH` at load time so the bundled binaries are visible to all child processes, not just the bash tool.

### Rust CLI fallback

The Rust CLI (`src/paths.rs`) resolves the scope with `resolve_scope_from`, which checks `AGENT_TERMINAL_SCOPE` first and then falls back to `PI_SESSION_ID` before the literal `standalone` scope. This means scope isolation works under pi even when the extension is not loaded — pi's native `PI_SESSION_ID` alone is sufficient. omp does not expose `PI_SESSION_ID` to bash commands, so under omp the extension is required for per-session isolation. Empty or whitespace-only values are treated as absent at every step.

### Flag registration

pi does not natively provide `--cwd`, `--no-lsp`, or `--no-context-files`, so the extension registers them with `pi.registerFlag` before any early return, ensuring they are always accepted regardless of whether the bundled binary is present. The `--cwd` flag selects the bash tool's working directory.

### Build script

`pi/npm/scripts/build.mjs` builds the Rust binary once (`x86_64-unknown-linux-musl`), compiles the extension with `bun build`, and copies both into each package. The `bun build` invocation passes `--external @earendil-works/pi-coding-agent` so the pi SDK is not bundled — the extension imports `createBashTool` and the extension API types from the host pi installation at runtime instead. The skill is shared with the OpenCode integration: `opencode/skills/` is copied verbatim into each package by `copySkillToPackage`, so there is a single source of truth for the skill content. The bundle variant's Zellij binary is content-pinned by SHA-256 and cached under `~/.cache/agent-terminal-zellij` for offline rebuilds.

### e2e gate

`scripts/e2e-pi-local.sh` is the deterministic release gate for the pi/omp integration, mirroring `scripts/e2e-opencode-local.sh`. `AGENT_TERMINAL_AGENT=pi|omp` selects the agent under test. Because pi/omp do not read a config file for custom providers, the fixture provider is registered through a small provider extension file loaded with `-e` alongside the agent-terminal extension. omp's bash tool appends `"Wall time: X seconds"` (and a `Command exited with code N` line) after the actual command output, so the fixture (`pi/scripts/e2e-fixture.ts`) and the strict verifier (`pi/scripts/e2e-verify.ts`) strip that suffix before extracting and matching the JSON payloads — this keeps the same strict transcript verification working for both agents.

In addition to the deterministic gate, `scripts/e2e-pi-real.sh` is an **optional, local-only** real-model e2e mirroring `scripts/e2e-opencode-real.sh`: it is not a release gate, and it drives the same lifecycle through the real pi/omp binary with a real model from the user's own pi config. It projects only the selected provider's model config (`models.json` for pi, `models.yml` for omp) and `auth.json` entries plus a minimal `settings.json` into a sandbox config directory (`PI_CODING_AGENT_DIR`), which is always deleted on exit, and runs the shared harness with `AGENT_TERMINAL_VERIFY_MODE=real` so the verifier's relaxed real-mode checks apply — the `E2E_SUCCESS` marker, a scope probe, and up to 8 tool-call attempts for observational `read`/`list` milestones — instead of the strict fixture transcript matching.
