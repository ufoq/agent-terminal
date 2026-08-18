# @ufoq/pi-agent-terminal-bundle-zellij

Bundles agent-terminal CLI + skill as a pi coding agent extension. Also bundles
a pinned Zellij 0.44.3 binary.

- Linux x86_64 `agent-terminal` binary
- `agent-terminal` skill for pi
- Pinned Zellij 0.44.3 binary (see [THIRD_PARTY.md](./THIRD_PARTY.md))
- pi extension entry point (`pi.extensions`)

## Install

```sh
npm install @ufoq/pi-agent-terminal-bundle-zellij
```

## Usage

Install the package and enable the extension in pi. The extension injects the
`AGENT_TERMINAL_SCOPE` environment variable into bash tool invocations, exposes
the bundled skill, and prepends the bundled Zellij binary to `PATH`.

## License

MIT
