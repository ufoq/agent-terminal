# @ufoq/opencode-agent-terminal

OpenCode plugin that bundles the Linux x86_64 `agent-terminal` binary and its skill.

This is the **slim** variant: it does **not** include a Zellij binary. The host must
have Zellij installed and available on `PATH`.

Install by adding an exact version to `opencode.json`:

```json
{
  "plugin": ["@ufoq/opencode-agent-terminal@0.1.3"]
}
```

Restart OpenCode after editing the config. The plugin registers the bundled skill
and exposes the bundled `agent-terminal` binary on Bash `PATH` for OpenCode shell
calls.

If you prefer a self-contained package that includes a pinned Zellij binary, use
`@ufoq/opencode-agent-terminal-bundle-zellij` instead.
