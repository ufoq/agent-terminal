# @ufoq/opencode-agent-terminal

OpenCode plugin that bundles the Linux x86_64 `agent-terminal` binary, its skill, and a pinned Zellij binary.

Install by adding an exact version to `opencode.json`:

```json
{
  "plugin": ["@ufoq/opencode-agent-terminal@0.1.1"]
}
```

Restart OpenCode after editing the config. The plugin registers the bundled skill and exposes both the bundled `agent-terminal` and the bundled `zellij` binaries on Bash `PATH`. No separate host install is required.

If the bundled Zellij is missing, the plugin falls back to any `zellij` available on the host `PATH`.
