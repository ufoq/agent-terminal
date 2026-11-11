# agent-terminal

Persistent terminal jobs for coding agents.

`agent-terminal` runs each job in its own Zellij pane, so a server, watcher, REPL, or
interactive command keeps running while the agent reads output, sends input, and presses
keys across turns. Jobs survive across agent turns, and a bounded 200-line screen keeps
every response small.

This repository ships one CLI and one integration: a pi extension plus its skill. The
bundle package includes a pinned Zellij binary.

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [CLI](#cli)
- [pi integration](#pi-integration)
- [Lifecycle](#lifecycle)
- [Scope and isolation](#scope-and-isolation)
- [End-to-end testing](#end-to-end-testing)
- [Optional real-model e2e](#optional-real-model-e2e)
- [Releasing](#releasing)
- [Development](#development)
- [License](#license)

## Requirements

- Linux x86_64
- [Zellij](https://zellij.dev) 0.44 or newer
- pi for the agent integration

The `-bundle-zellij` packages ship a pinned Zellij binary, so no system Zellij install is
needed for those; the plain packages require `zellij` on `PATH`. Builds need Rust, Bun,
and Python 3. Runtime needs only the packaged binary and Zellij.

## Install

### CLI

```sh
cargo install --path .
# or build a static binary
cargo build --release --target x86_64-unknown-linux-musl
```

### pi extension and skill

```sh
pi install npm:@ufoq/pi-agent-terminal-bundle-zellij
# or, without the bundled Zellij binary
pi install npm:@ufoq/pi-agent-terminal
```

The extension registers the `agent-terminal` skill and puts its packaged binary on
`PATH`, so the agent discovers the commands without any env setup.

## CLI

```text
agent-terminal start <NAME> [--cwd DIR] [--detached] [--json] -- <COMMAND...>
agent-terminal read <NAME> [--json]
agent-terminal send <NAME> [--json] -- <TEXT...>
agent-terminal press <NAME> [--json] -- <KEY...>
agent-terminal stop <NAME> [--json]
agent-terminal list [--json]
```

- `start` creates a named job in its own Zellij pane and runs the command inside it.
  `--cwd` changes the working directory; it is only valid on `start`.
- `read` returns the current 200-line screen and job state. Read it before resending
  input.
- `send` writes literal text to the job's pane.
- `press` sends named keys such as `Enter`, `Tab`, or `Escape`.
- `stop` stops the job and removes it.
- `list` lists jobs in the current scope.

The `--json` envelope is stable for programmatic use:

```json
{
  "status": "ok",
  "jobs": [
    {
      "name": "dev",
      "state": "running",
      "cwd": "/home/me/project",
      "last_activity": "2026-01-15T10:23:45Z",
      "exit_code": null
    }
  ]
}
```

`read` adds a `screen` field and the job's `state`, `exit_code`, and `last_activity`;
`start`, `send`, `press`, and `stop` return the corresponding status acknowledgement.
Errors are returned as `{"status": "error", "error": "...", "message": "..."}` with a
non-zero exit code.

## pi integration

The pi package is loaded through pi's package manager:

```sh
pi install npm:@ufoq/pi-agent-terminal-bundle-zellij
```

Or from a local package root:

```sh
pi install ./pi/npm/packages/pi-agent-terminal-bundle-zellij
```

The package declares its extension and skill in the `pi` manifest field:

```json
{
  "pi": {
    "extensions": ["./dist/index.js"],
    "skills": ["./skills"]
  }
}
```

The extension only prepares `PATH` for the Bash tool and registers the compatibility
flags. It never replaces or wraps pi's bash tool. The skill is discovered statically from
the package root; loading `dist/index.js` directly is extension-only and does not expose
the skill.

## Lifecycle

A job lives in its own Zellij pane:

```sh
agent-terminal start dev -- npm run dev
agent-terminal read dev
agent-terminal send dev -- "hello"
agent-terminal press dev -- Enter
agent-terminal read dev
agent-terminal stop dev
```

- `start` creates a pane and starts the command.
- The job's state is `running` until the command exits, then `exited` with its exit code.
- `read` returns the bounded screen; the pane keeps running between reads.
- `send` writes text without pressing Enter; `press` sends the key.
- `stop` stops the process and removes the job.
- Jobs are keyed to the project root containing the current working directory.

A job that finishes stays visible until `stop` removes it. `list` shows the current
scope's jobs.

## Scope and isolation

State is isolated per agent session and per project. The CLI resolves scope from
`AGENT_TERMINAL_SCOPE`, then pi's `PI_SESSION_ID`, then the literal `standalone`. That
means two concurrent pi sessions in the same project cannot see each other's jobs, while
repeated calls from one session address the same jobs.

The state root is `~/.local/state/agent-terminal` (or
`$XDG_STATE_HOME/agent-terminal`). Jobs for a scope live under
`<state-root>/scopes/<scope-digest>/projects/<project-digest>/`, alongside the private
`config.kdl` passed to Zellij. That directory is created `0700`.

## End-to-end testing

The deterministic gate uses a local OpenAI-compatible fixture server and a real pi binary:

```sh
cd pi
bun run e2e:pi
```

It covers the full lifecycle: list, start, read, send, press, stop, and list again. The
fixture replaces only the model's reasoning; the pi extension, Bash execution, Zellij, and
agent-terminal are real.

Additional gates:

```sh
cd pi
bun run e2e:pi:skip-prompt   # direct CLI smoke only
bun run e2e:pi:skills        # static skill discovery through pi
```

The two-session isolation gate runs two fixtures against one project:

```sh
bash scripts/e2e-two-session.sh
```

The gates create private state roots under `/tmp`, load the bundle package, and clean up
after themselves. Set `AGENT_TERMINAL_CLEANUP=0` to keep the artifacts.

## Optional real-model e2e

`scripts/e2e-pi-real.sh` is a local-only, real-model check. It is not a release gate and
requires a provider/model from your own pi config:

```sh
PI_MODEL=litellm/ollama-cloud/deepseek-v4-flash \
  bash scripts/e2e-pi-real.sh
```

It projects only the selected provider's `models.json` and `auth.json` entries into a
sandbox config directory, drives the same lifecycle through a real model, and deletes the
sandbox config on exit.

## Releasing

The two pi packages are versioned together from `release.json`:

```sh
node scripts/release-version.mjs check
node scripts/release-version.mjs sync 0.2.0
```

Then build and publish serially:

```sh
cd pi
bun run build
npm publish npm/packages/pi-agent-terminal
npm publish npm/packages/pi-agent-terminal-bundle-zellij
```

Publishing is intentionally serial: concurrent publishes can hit registry rate limits.
Allow a few minutes for CDN propagation before installing the new version elsewhere.

## Development

Repository layout:

- `src/` — Rust CLI and the Zellij backend.
- `pi/` — pi extension, skill source, npm packages, and e2e fixtures.
- `scripts/` — shared build, release, and e2e helpers.
- `docs/design.md` — the implementation contract.

Useful commands:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cd pi && bun run check
```

`bun run check` runs formatting, linting, type checks, tests, and the bundle build.

## License

MIT
