import { describe, expect, test } from "bun:test"
import { readFile } from "node:fs/promises"
import { resolve } from "node:path"

const projectRoot = resolve(import.meta.dir, "../..")
const skillPath = resolve(projectRoot, "opencode/skills/agent-terminal/SKILL.md")

describe("agent-terminal skill bundle", () => {
  test("uses valid focused frontmatter", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill.startsWith("---\n")).toBe(true)
    const closingMarker = skill.indexOf("\n---\n", 4)
    expect(closingMarker).toBeGreaterThan(4)

    const frontmatter = skill.slice(4, closingMarker)
    expect(frontmatter).toContain("name: agent-terminal")
    const description = frontmatter.split("\n").find((line) => line.startsWith("description: "))
    expect(description).toBeDefined()
    expect(description?.length ?? 0).toBeLessThanOrEqual(1_024)
  })

  test("teaches all tools and routing without leaking backend commands", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const toolName of [
      "terminal_start",
      "terminal_read",
      "terminal_send",
      "terminal_press",
      "terminal_stop",
      "terminal_list",
    ]) {
      expect(skill).toContain(toolName)
    }

    expect(skill).toMatch(/(?:Bash.*short foreground|short foreground.*Bash)/s)
    expect(skill).toContain("job_still_running")
    expect(skill).toContain("force=true")
    expect(skill.toLowerCase()).not.toContain("zellij action")
    expect(skill).not.toContain("pane-id")
    expect(skill).not.toMatch(/terminal_(?:start|read|send|press|stop|list)\(/)
    expect(skill.split("\n").length).toBeLessThan(500)
  })

  test("repository layout can be copied directly into OpenCode config", async () => {
    const adapter = await readFile(resolve(projectRoot, "opencode/tools/terminal.ts"), "utf8")
    const readme = await readFile(resolve(projectRoot, "README.md"), "utf8")
    expect(adapter).toContain("export const start = tool(")
    expect(readme).toContain("opencode/tools")
    expect(readme).toContain("opencode/skills")
  })
})
