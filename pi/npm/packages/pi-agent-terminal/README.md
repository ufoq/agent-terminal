# @ufoq/pi-agent-terminal

Bundles agent-terminal CLI + skill as a pi coding agent extension.

- Linux x86_64 `agent-terminal` binary
- `agent-terminal` skill for pi
- pi extension entry point (`pi.extensions`)

## Install

```sh
npm install @ufoq/pi-agent-terminal
```

## Usage

Install the package and enable the extension in pi. The extension injects the
`AGENT_TERMINAL_SCOPE` environment variable into bash tool invocations and
exposes the bundled skill.

## License

MIT
