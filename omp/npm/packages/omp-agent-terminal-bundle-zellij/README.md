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

Install the package and enable the extension in omp:

```sh
omp -e npm:@ufoq/omp-agent-terminal-bundle-zellij
```

The extension revises bash tool calls: it defaults `AGENT_TERMINAL_SCOPE` to the
omp session id (preserving explicit overrides) and prepends the bundled
agent-terminal and Zellij binaries to the bash `env.PATH`.

When passing a local `-e` path instead, point it at the package root (the
directory containing `package.json`), not `dist/index.js`, so that the native
`skills/` sibling discovery runs.

## License

MIT
