# @ufoq/omp-agent-terminal-bundle-zellij

Bundles agent-terminal CLI + skill as an omp coding agent extension. Also
bundles a pinned Zellij 0.44.3 binary.

- Linux x86_64 `agent-terminal` binary
- `agent-terminal` skill for omp (native omp-plugins skill discovery)
- Pinned Zellij 0.44.3 binary (see [THIRD_PARTY.md](./THIRD_PARTY.md))
- omp extension entry point (`pi.extensions` manifest, honored by omp)

## Install

```sh
npm install @ufoq/omp-agent-terminal-bundle-zellij
```

## Usage

Install the package and enable the extension in omp. The extension revises bash
tool calls: it defaults `AGENT_TERMINAL_SCOPE` to the omp session id (preserving
explicit overrides) and prepends the bundled agent-terminal and Zellij binaries
to the bash `env.PATH`.

## License

MIT
