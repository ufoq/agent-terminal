# @ufoq/omp-agent-terminal

Bundles agent-terminal CLI + skill as an omp coding agent extension.

- Linux x86_64 `agent-terminal` binary
- `agent-terminal` skill for omp (native omp-plugins skill discovery)
- omp extension entry point (`pi.extensions` manifest, honored by omp)

## Install

```sh
npm install @ufoq/omp-agent-terminal
```

## Usage

Install the package and enable the extension in omp. The extension revises bash
tool calls: it defaults `AGENT_TERMINAL_SCOPE` to the omp session id (preserving
explicit overrides) and prepends the bundled binaries to the bash `env.PATH`.

## License

MIT
