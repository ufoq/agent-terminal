# @ufoq/pi-agent-terminal-bundle-zellij

Bundles the agent-terminal CLI and skill as a pi coding agent package, plus a
pinned Zellij 0.45.1 binary.

- Linux x86_64 `agent-terminal` binary
- `agent-terminal` skill for pi, declared via the package manifest (`pi.skills`)
- Pinned Zellij 0.45.1 binary (see [THIRD_PARTY.md](./THIRD_PARTY.md))
- pi extension entry point (`pi.extensions`)

## Install

```sh
# Persist globally; use `-l` for project-local .pi/settings.json.
pi install npm:@ufoq/pi-agent-terminal-bundle-zellij

# Or load temporarily for one run.
pi -e npm:@ufoq/pi-agent-terminal-bundle-zellij
```

Pi reads both the extension and bundled skill from the package manifest, so the skill is
discovered statically from `pi.skills` — no runtime skill registration is involved.

The extension itself is responsible for PATH setup and compatibility flags:

- It prepends the bundled `agent-terminal` binary directory to `PATH` so the
  CLI is available to every child process.
- It also prepends the bundled Zellij binary directory to `PATH`, so the pinned
  Zellij 0.45.1 takes precedence over any system Zellij.

This PATH change is process-local: it applies inside `pi` and its children, not
to a plain shell or `docker exec`. Outside `pi`, invoke the packaged absolute
`bin/linux-x64/agent-terminal` and add `--global` when there is no `pi`
ancestor. If Zellij is not on `PATH` outside `pi`, point
`AGENT_TERMINAL_ZELLIJ` at this package's `bin/zellij/zellij`.
- It registers `--cwd` and `--no-lsp` as accepted compatibility flags for the
  shared agent-terminal harness; pi has no native equivalents, so they are
  accepted without claiming behavior.

Jobs are scoped to the nearest `pi` process in the CLI's ancestor chain. The
full chain is recorded so a parent `pi` exit also cleans jobs started by
subagents. No environment variable is required; use `--global` for a
worker-lifetime namespace.
## License

MIT
