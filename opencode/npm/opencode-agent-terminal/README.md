# @ufoq/opencode-agent-terminal

OpenCode plugin that bundles the Linux `agent-terminal` binary and its skill.

Install by adding an exact version to `opencode.json`:

```json
{
  "plugin": ["@ufoq/opencode-agent-terminal@0.1.0"]
}
```

Restart OpenCode after editing the config. The plugin registers the bundled skill and exposes the matching Linux binary on Bash `PATH` for OpenCode shell calls.

Requires Zellij on the host.
