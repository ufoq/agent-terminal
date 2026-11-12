# agent-terminal 0.2.0 design

0.2.0 supports pi only. The interaction surface is a Rust CLI driven from Bash, plus the
`agent-terminal` skill. There is no native pi tool, no MCP server, and no extra command.

## Goals

The tool helps an agent keep terminal work across turns. It must be simple, reliable, and
isolated enough that agents do not lose control of their own jobs. It must not leak
processes or state during long sessions.

Isolation is operational, not a security boundary. The worker is already sandboxed, and
any agent with Bash can reach the filesystem and processes. Scopes prevent accidental
collisions and wrong cleanup, not deliberate interference.

## Ownership

The CLI walks `/proc` from its own process to the root:

```text
pi
└── bash tool
    └── agent-terminal
```

- A process is pi when `/proc/<pid>/comm` is `pi`, its executable basename is `pi`, or its
  argv contains a `pi` path.
- The nearest pi ancestor is the owner.
- Every pi ancestor, nearest to root, is kept for cleanup.
- Identity is `(pid, starttime)` from `/proc/<pid>/stat`. A zombie, missing process, or
  changed starttime is dead.
- No pi ancestor is an error that points at `--global`.

Ownership files contain one `pid:starttime` per ancestor and are written before the
Zellij session is created. They are not rewritten while the owner is alive.

## Scope

A scope is the project digest plus the owner identity. The project is the nearest `.git`
ancestor, or the invocation directory. Parent pi, every subagent child pi, and siblings
therefore have separate job namespaces automatically.

`--global` selects a per-project worker-lifetime namespace shared by every pi process in
that project. It has no owner watcher. It is removed by an explicit stop once no job or
unknown panes remain, after 60 seconds with no job or unknown panes, or when the worker
stops.

## Jobs

A job is a Zellij pane titled `agent-terminal:job:<name>`. There is no persisted job
registry and no `state.json`. `list` reads pane titles; `read`, `send`, `press`, and
`stop` resolve the pane directly.

Job names match `[a-z0-9][a-z0-9._-]{0,63}`.

Jobs are created with `--close-on-exit`, so an exited process leaves no pane, no job
record, and no stale state. A completed job simply disappears. The skill tells the agent
to capture output and exit status in a file inside the job command instead of racing to
read before the process exits, and to stop jobs it no longer needs.

If a project has state but its named session is absent, diagnostic commands
report it: `list` returns an empty `jobs` array plus `"session":"lost"` when no
job panes are visible, and `read`/`send`/`press` return `session_lost` instead
of `job_not_found`. `list` is the one diagnostic that does not sweep the
project, so an immediately following `read` still sees the absence and reports
it too. The next non-`list` command sweeps the state, so the observation is not
durable. It is not proof that a named job existed: failed-start residue, keeper
idle cleanup, or a wrong project with stale state can produce it. `start` and
`stop` skip the observation and keep their recovery paths. A session that
disappears after the observation is not relabelled. There is still no registry
or tombstone.

Backend failure logging is content-free: it records exit status, argument count,
parse line/column, and captured output lengths, never command arguments, raw
stderr, or a parser error string. `paste` text and `new-pane` commands therefore
cannot leak through diagnostics.

## Sessions and keeper

Each project in an owner scope has one Zellij session:

```text
at-<project-digest-12>-<scope-digest-12>
```

Global sessions are `at-global-<project-digest-12>`.

Session creation uses a private layout containing one pane:

```text
agent-terminal:keeper
```

The keeper is the hidden command:

```text
agent-terminal __keeper <owners> <ready> <zellij> <lock>
```

For `--global`, `<owners>` and `<ready>` are `-`.

The keeper:

1. Reads the owner chain once.
2. Writes `ready` with its pane id.
3. Checks owner liveness about once per second.
4. When any owner or ancestor is dead, runs `zellij kill-session` for its own session.
5. Never kills by PID and never deletes state.

If the owners file is missing or malformed, the keeper kills its own session. It cannot
verify ownership, and a session it cannot attribute must not outlive it. The CLI sweep
removes the state afterwards.

The keeper also removes an idle session after 60 seconds with no job panes, under the
project lock and only when there are no unknown panes. A non-global stop of the last job
removes the session immediately.

## Start order

```text
lock project
sweep
write owners (owner mode)
write config.kdl and layout.kdl
create Zellij session and keeper pane
wait for keeper pane and ready
create job pane with --name and --close-on-exit
```

A crash after session creation leaves a session with only a keeper; the next command's
sweep removes it. A crash after pane creation leaves a normal job pane.

## Sweep

Sweep runs before every command under the project lock, except a `list` that
reports session loss: that `list` preserves the state for one following
diagnostic, and the next non-`list` command sweeps. Projects in other scopes or
with other digests are inspected only after taking their own non-blocking project lock;
busy targets are skipped.

- Dead owner scope: kill the project session, verify absence, remove the project
  directory.
- Current live owner: if the session is absent, remove stale state; if the session has no
  job panes and no unknown panes, kill it and remove state.
- Global: remove an idle session and its project state.
- Unknown non-plugin panes prevent live-owner recovery and idle cleanup. Dead-owner
  cleanup may remove the session without inspecting panes.
- Scratch is cleared per project.

The per-project lock lives at `<runtime>/locks/<project>.lock`, outside every scope
directory. It is never deleted, so a later process opens the same inode. A short-held
`<runtime>/.meta.lock` serializes project-directory creation and empty-scope pruning, so
one project cannot remove a shared parent while another project is being prepared.

## Config isolation

Zellij never reads or writes the user's real configuration.

- `ZELLIJ_CONFIG_DIR` points at the private project directory containing `config.kdl`.
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, and `XDG_CACHE_HOME` point inside the private
  runtime root.
- `ZELLIJ_CONFIG_FILE` is removed from the child environment.
- `--config` is not passed.

Private directories are `0700`; private files are `0600`.

## Runtime root

`--state-dir` wins. Otherwise:

```text
$XDG_RUNTIME_DIR/agent-terminal
```

or

```text
/tmp/agent-terminal-<uid>
```

The root must be a real directory owned by the effective uid. Symlinks are rejected and
the mode is set to `0700`.

## Locking

One lock per project, acquired for the whole command, with 50 ms retries up to 30 seconds.
Contention returns `lock_busy` after the budget. The lock is not used to protect Zellij
reads; it serializes cleanup and mutations.

## CLI and output

Six commands: `start`, `read`, `send`, `press`, `stop`, `list`.

- `start` takes `--cwd`, only on `start`.
- `send` always submits with Enter.
- `press` sends one key.
- `--project`, `--state-dir`, `--global`, `--pretty`, and `-v` are global options.

Every response is one JSON object with `status: ok|error`, stable error codes, and no
state paths.

`read` returns the newest 200 lines or 32 KiB, whichever is smaller, with `truncated`
describing only that inline preview.

`read --save PATH` writes the complete ANSI-stripped retained scrollback to `PATH` and
adds `saved` (absolute path), `saved_bytes`, and `saved_lines` to the response. The inline
preview stays bounded. The destination must name a file whose parent directory already exists;
it is replaced atomically with mode `0600`, and is not followed through a final symlink. The saved file
is a caller-owned snapshot, not a live tail, and is not removed by `stop`. A save I/O failure
(missing parent, permissions, full disk, rename) is `state_io` and names the caller-supplied
destination; malformed paths are `invalid_input`, and runtime state paths stay hidden.
The 32 KiB cap applies only to the preview.

## Subagents

No coupling to `pi-mini-subagents`.

```text
parent pi
└── child pi
    └── bash
        └── agent-terminal
```

- Each child pi is its own owner and gets its own scope.
- The child keeper watches the child and all higher pi ancestors, so a parent crash also
  cleans child jobs.
- `subagent remove` kills the child pi; its keeper removes the child session.
- Parent and siblings cannot see each other's attached jobs.
- `--global` is the only deliberate namespace that outlives `pi`.

## Deliberate non-goals

- No omp or OpenCode support.
- No session-id or environment-variable scope.
- No persisted job registry.
- No daemon or separate garbage collector.
- No public cleanup command.
- No socket-directory colocation.
- No per-child scope injection.
- No `status` or `new` aliases.
- No native tool or MCP.

## Residuals

A job that deliberately double-forks or calls `setsid` can escape the Zellij session.
Strict containment needs cgroups, PID namespaces, or systemd, which this worker does not
provide. The supported contract is that job commands stay in their pane.
