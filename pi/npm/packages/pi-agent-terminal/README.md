# @ufoq/pi-agent-terminal

Bundles the agent-terminal CLI and skill as a pi coding agent package.

- Linux x86_64 `agent-terminal` binary
- `agent-terminal` skill for pi, declared via the package manifest (`pi.skills`)
- pi extension entry point (`pi.extensions`)

## Install

```sh
# Persist globally; use `-l` for project-local .pi/settings.json.
pi install npm:@ufoq/pi-agent-terminal

# Or load temporarily for one run.
pi -e npm:@ufoq/pi-agent-terminal
```

Pi reads both the extension and bundled skill from the package manifest, so the skill is
discovered statically from `pi.skills` — no runtime skill registration is involved.

The extension itself is responsible for PATH setup and compatibility flags:

- It prepends the bundled `agent-terminal` binary directory to `PATH` so the
  CLI is available to every child process.
- It registers `--cwd` and `--no-lsp` as accepted compatibility flags for the
  shared agent-terminal harness; pi has no native equivalents, so they are
  accepted without claiming behavior.

Jobs are scoped to the nearest `pi` process in the CLI's ancestor chain. The
full chain is recorded so a parent `pi` exit also cleans jobs started by
subagents. No environment variable is required; use `--global` for a
worker-lifetime namespace.
## License

MIT
