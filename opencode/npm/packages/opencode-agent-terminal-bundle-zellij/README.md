# @ufoq/opencode-agent-terminal-bundle-zellij

OpenCode plugin that bundles the Linux x86_64 `agent-terminal` binary, its skill,
and a pinned Zellij binary.

This is the **bundle** variant: it does **not** require Zellij to be installed on
the host. The plugin prepends its bundled `zellij` binary to Bash `PATH` so that
`agent-terminal` uses it automatically.

Install by adding an exact version to `opencode.json`:

```json
{
  "plugin": ["@ufoq/opencode-agent-terminal-bundle-zellij@0.1.3"]
}
```

Restart OpenCode after editing the config. The plugin registers the bundled skill
and exposes both the bundled `agent-terminal` and the bundled `zellij` binaries on
Bash `PATH`.

If you already have Zellij installed and want a smaller package, use
`@ufoq/opencode-agent-terminal` instead.
