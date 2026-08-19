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

Each host gets its own adapter extension and its own pair of npm packages, built by
per-host workspace scripts over the shared build library `scripts/npm-build-lib.mjs`
(which keeps the Zellij version and SHA-256 pins in one place):

| package | host | contents |
|---|---|---|
| `@ufoq/pi-agent-terminal` | pi | adapter, static Linux x86_64 `agent-terminal` binary, skill; host must have Zellij on `PATH` |
| `@ufoq/pi-agent-terminal-bundle-zellij` | pi | same, plus a pinned Zellij binary |
| `@ufoq/omp-agent-terminal` | omp | adapter, binary, skill; host must have Zellij on `PATH` |
| `@ufoq/omp-agent-terminal-bundle-zellij` | omp | same, plus a pinned Zellij binary |

Pi packages declare their extension entry point and skill directory in the `pi` manifest
field (`"pi": { "extensions": ["./dist/index.js"], "skills": ["./skills"] }`), which
pi honors. OMP packages declare only their extension entry point; omp's native
`omp-plugins` provider discovers the shipped `skills/` sibling when the package root
loads. Pi packages load via pi's `-e` flag or auto-discovery from
`~/.pi/agent/extensions/`; omp packages via omp's `-e` flag or auto-discovery from
`~/.omp/agent/extensions/`. The Rust CLI, skill content, bundled binaries, and JSON
protocol are unchanged and shared between hosts.

The build pipeline compiles the Rust binary once (`x86_64-unknown-linux-musl`), compiles
the per-host adapter with `bun build`, and copies the binary, the dist, and the skill into
each package. The skill is shared with the OpenCode integration:
`opencode/skills/` is the single source of truth and is copied verbatim by
`copySkillToPackage`. The bundle variant's Zellij binary is content-pinned by SHA-256 and
cached under `~/.cache/agent-terminal-zellij` for offline rebuilds; both bundle
entrypoints `await` `ensureZellij` before finishing, so success is never reported before
the pinned binary is installed and verified.

### pi adapter

The pi adapter never touches the bash tool. Pi's native bash keeps its stock definition,
approval, and concurrency by construction — the adapter neither replaces the tool nor
wraps its execution. On load it guards the platform (`linux`) and then the architecture
(`x64`) first, returning early on either mismatch; next it registers the compatibility
flags; only the PATH mutation is gated behind the bundled-binary check:

1. Registers the compatibility flags `--cwd` and `--no-lsp`, which pi does not provide
   natively. These are honest compatibility registrations: the adapter makes no behavior
   claims, and `--cwd` no longer feeds a bash tool (bash's working directory is the
   invocation directory). `--no-context-files` is native to pi and is not registered.
2. Checks the bundled `agent-terminal` executable; if it is missing or not executable, the
   adapter logs a diagnostic and returns before mutating PATH.
3. Mutates `process.env.PATH` with the ordering `bundledDirs : piManagedBin : rest`
   (deduplicated). `piManagedBin` is `join(getAgentDir(), "bin")` — the SDK's root
   `getAgentDir` export, which honors `PI_CODING_AGENT_DIR`; the SDK does not export
   `getBinDir`. Because pi's `getShellEnv` live-spreads `process.env` and only prepends
   `~/.pi/agent/bin` when it is absent, the explicit insertion makes the managed-bin check
   pass and keeps the bundled directories first — a stale `agent-terminal`/`zellij` in the
   managed bin can never shadow the bundled ones.

The adapter registers no skill handler. The shared skill is discovered statically by the
package resource loader: the `pi.skills` manifest entry (`["./skills"]`) exposes it for
package-root loads (`-e <package root>` or `npm:`), while loading the dist file directly
(`-e <package>/dist/index.js`) is extension-only and intentionally does not discover the
skill.

Scope: pi injects `PI_SESSION_ID` into every bash child natively, so per-session isolation
needs zero extension code. The Rust CLI resolves `AGENT_TERMINAL_SCOPE` → `PI_SESSION_ID`
→ `standalone` (empty/whitespace values treated as absent at every step), so pi gets
per-session isolation even when the extension is not loaded.

### omp adapter

The omp adapter (`omp/npm/src/index.ts`, zero runtime imports, hand-written local types for
the documented extension surface) does not replace omp's bash tool either.
Instead it registers exactly one `pi.on("tool_call")` input-revision handler: for a `bash`
tool call it returns a revised `input` whose `env` carries the injected defaults, and
returns nothing for every other tool. Because the revision happens at argument-prep time,
the revised input is revalidated and flows through native schema checks, concurrency
scheduling, the approval gate, and the persisted assistant message alike. Native bash
definition, approval, concurrency, conditional `async` schema, strict mode, PTY, and direnv
preflight are therefore preserved by construction. No tool is registered and no tool
invocation is emulated: the native tool surface runs as-is with no schema mirroring.

The handler composes the env defaults in this order:

1. Scope: when the call's `env.AGENT_TERMINAL_SCOPE` is undefined, empty, or
   whitespace-only, it defaults to `ctx.sessionManager.getSessionId()` (guarded with
   `typeof` and non-empty), because omp injects no session environment into bash children.
   An explicit value always wins — including a shell-level
   `AGENT_TERMINAL_SCOPE=shared cmd` assignment, since bash env values are environment
   values, not shell text, and the child's explicit env overrides the inherited default.
   Without a session manager or session id, the scope is left unset (the CLI falls back to
   `standalone`) and a one-time diagnostic is emitted.
2. PATH: the revision branch applies only to valid env inputs — a non-null, non-array
   object whose values are all strings. `env.PATH` is the bundled binary directories
   prepended to the call's supplied `env.PATH`, or to `process.env.PATH` when the call
   carries none (deduplicated). A supplied value (including an explicit empty string,
   which yields the bundled dirs alone) replaces the process PATH rather than inheriting
   it, so the process PATH is the base only when the call carries no `env.PATH`. Host
   binaries (e.g. `printenv`, `git`) stay resolvable through that base. A malformed env
   object (null, array, or non-string values) is returned unrevised so omp's native bash
   schema validation reports it.

The adapter registers the compatibility flag `--no-context-files` only, which omp does not
provide natively; `--no-lsp` and `--cwd` are native to omp and are not registered.

Skill: omp's `omp-plugins` provider discovers `skills/<name>/SKILL.md` natively for
packages loaded through `-e <package root>` or the `npm:` spec (with
`requireDescription: true`, which our SKILL.md frontmatter satisfies), so the omp adapter
registers no skill handler.

### Flag matrix

| flag | pi | omp |
|---|---|---|
| `--cwd` | adapter-registered compatibility flag | native |
| `--no-lsp` | adapter-registered compatibility flag | native |
| `--no-context-files` | native | adapter-registered compatibility flag |

### e2e gates

`scripts/e2e-pi-local.sh` is the deterministic release gate for the per-agent integration,
mirroring `scripts/e2e-opencode-local.sh`. `AGENT_TERMINAL_AGENT=pi|omp` selects the agent
under test, its package (`pi/npm/packages/pi-agent-terminal-bundle-zellij` or
`omp/npm/packages/omp-agent-terminal-bundle-zellij`), its workspace build, and its probes:

- pi: `printenv PI_SESSION_ID`, asserted equal to the session header id.
- omp: `printenv AGENT_TERMINAL_SCOPE` (asserted equal to the session header id), then
  `AGENT_TERMINAL_SCOPE=shared printenv AGENT_TERMINAL_SCOPE` (asserted `shared`).

Because pi/omp do not read a config file for custom providers, the fixture provider is
registered through a small provider extension file loaded with `-e` alongside the
agent-terminal extension. omp's bash tool appends `"Wall time: X seconds"` (and a
`Command exited with code N` line) after the actual command output, so the fixture
(`pi/scripts/e2e-fixture.ts`) and the strict verifier (`pi/scripts/e2e-verify.ts`, with
`--agent pi|omp`) strip that suffix before extracting and matching the JSON payloads.

`scripts/e2e-two-session.sh` is a deterministic two-session isolation gate: two fixture
servers (`AGENT_TERMINAL_FIXTURE_HOLD=1`, holding jobs open for
`AGENT_TERMINAL_FIXTURE_HOLD_SECS`, default 45 seconds) drive two concurrent agent runs
against one shared project directory, state root, and Zellij socket dir. It asserts the two
session header ids differ, direct-CLI cross-checks (`agent-terminal list`/`read server`)
under each scope see exactly their own job and pane during the hold window, and both
sessions finish the full lifecycle with empty final lists. `AGENT_TERMINAL_JOB_NAME` keeps
both runs on the same job name. This exercises the exact per-host scope mechanism: pi via
native `PI_SESSION_ID` → Rust CLI fallback, omp via the adapter's `tool_call` revision →
Rust CLI priority.

`scripts/e2e-skill-discovery.sh` is a unified skill-discovery smoke: with
`AGENT_TERMINAL_FIXTURE_SKILL_PROBE=1`, the mini fixture inspects the first provider
request's messages and asserts the skill's DESCRIPTION phrase ("Run persistent or
interactive terminal jobs through a simple Zellij wrapper") is present — hosts inject skill
metadata (name + description + location), not body text, so the description is the
discovery marker. Both runs load the package with `-e <package root>` (not the dist
file). Pi's package resource loader reads the declared `pi.skills` directory there;
omp's native `omp-plugins` provider discovers the shipped `<package>/skills` sibling
there. This proves real-host static skill discovery end to end on both hosts: discovery
reaches the model request as skill metadata.

In addition to the deterministic gates, `scripts/e2e-pi-real.sh` is an **optional,
local-only** real-model e2e mirroring `scripts/e2e-opencode-real.sh`: it is not a release
gate, and it drives the same lifecycle through the real pi/omp binary with a real model
from the user's own pi config, loading the per-agent package. It projects only the selected
provider's model config (`models.json` for pi, `models.yml` for omp) and `auth.json` entries
plus a minimal `settings.json` into a sandbox config directory (`PI_CODING_AGENT_DIR`),
which is always deleted on exit, and runs the shared harness with
`AGENT_TERMINAL_VERIFY_MODE=real` so the verifier's relaxed real-mode checks apply — the
`E2E_SUCCESS` marker, a scope probe, and up to 8 tool-call attempts for observational
`read`/`list` milestones — instead of the strict fixture transcript matching.

### Known caveats

- **direnv PATH**: on direnv projects, an explicit bash `env.PATH` takes precedence over
  the `.envrc` PATH (omp's native caller-env-over-direnv precedence). The bundled binaries
  are always prepended; the process PATH is the base only when the call supplies no
  `env.PATH`.
- **Last-wins revision composition**: `tool_call` input revisions compose across extensions
  with the last returned input winning, and handlers do not observe each other's revisions.
  Another extension revising bash input may override this adapter's revision or vice versa —
  strictly better than the tool-registration conflict the split removes.
