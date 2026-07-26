import { describe, expect, test } from "bun:test"
import { readFile, access } from "node:fs/promises"
import { resolve } from "node:path"

const projectRoot = resolve(import.meta.dir, "../..")
const skillPath = resolve(projectRoot, "opencode/skills/agent-terminal/SKILL.md")

describe("agent-terminal CLI skill contract", () => {
  test("has valid frontmatter with required fields", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill.startsWith("---\n")).toBe(true)
    const closingMarker = skill.indexOf("\n---\n", 4)
    expect(closingMarker).toBeGreaterThan(4)

    const frontmatter = skill.slice(4, closingMarker)
    expect(frontmatter).toContain("name: agent-terminal")
    const description = frontmatter.split("\n").find((line) => line.startsWith("description: "))
    expect(description).toBeDefined()
    expect(description?.startsWith('description: "')).toBe(true)
    expect(description?.length).toBeLessThanOrEqual(1_024)
    expect(frontmatter).toContain("compatibility:")
    expect(frontmatter).toContain("project: agent-terminal")
    expect(frontmatter).toContain('version: "1"')
  })

  test("teaches CLI commands with --project for all six verbs", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const verb of ["start", "read", "send", "press", "stop", "list"]) {
      expect(skill).toContain(`agent-terminal --project "$PROJECT" ${verb}`)
    }
  })

  test("uses --cwd for start and documents /bin/sh -c for shell syntax", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toMatch(/start.*--cwd/)
    expect(skill).toContain('--cwd "$CWD"')
    expect(skill).toContain("/bin/sh -c")
  })

  test("documents --no-submit and --force flags", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("--no-submit")
    expect(skill).toContain("--force")
  })

  test("derives PROJECT from git rev-parse and CWD from pwd -P", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("git rev-parse --show-toplevel")
    expect(skill).toContain('CWD="$(pwd -P)"')
  })

  test("notes PROJECT and CWD do not persist across Bash calls", async () => {
    const skill = await readFile(skillPath, "utf8")
    const lower = skill.toLowerCase()
    const mentionsNonPersistence = [
      "do not persist",
      "does not persist",
      "re-derived",
      "re-derive",
    ].some((phrase) => lower.includes(phrase))
    expect(mentionsNonPersistence).toBe(true)
  })

  test("documents JSON envelope semantics", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const term of ["status", "ok", "error", "data", "code", "message", "hint"]) {
      expect(skill).toContain(term)
    }
  })

  test("documents exit code semantics", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toMatch(/\| 1 \|/)
    expect(skill).toMatch(/\| 2 \|/)
    expect(skill).toContain("Expected job or lock error")
    expect(skill).toContain("Invalid input")
  })

  test("documents lifecycle states running exited lost", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const state of ["running", "exited", "lost"]) {
      expect(skill).toContain(state)
    }
  })

  test("documents all five recovery codes", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const code of [
      "job_exists",
      "job_not_found",
      "job_not_running",
      "job_still_running",
      "lock_busy",
    ]) {
      expect(skill).toContain(code)
    }
  })

  test("documents job name grammar", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("[a-z0-9]")
    expect(skill).toContain("{0,63}")
  })

  test("documents accepted named key grammar", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const key of [
      "Enter",
      "Tab",
      "Esc",
      "Backspace",
      "Delete",
      "Insert",
      "Home",
      "End",
      "PageUp",
      "PageDown",
      "Up",
      "Down",
      "Left",
      "Right",
    ]) {
      expect(skill).toContain(key)
    }
    expect(skill).toMatch(/F1[\s\S]*F12/)
    expect(skill).toContain("Ctrl+")
    expect(skill).toContain("Alt+")
  })

  test("distinguishes persistent jobs from foreground Bash", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("persistent")
    const lower = skill.toLowerCase()
    expect(lower).toContain("foreground")
  })

  test("teaches screen boundedness and state-over-screen rule", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("truncated")
    expect(skill).toContain("State, not screen activity, determines completion")
    expect(skill.toLowerCase()).toContain("read before")
  })

  test("documents cleanup and cancellation rules", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("authoritative")
    expect(skill).toContain("Cancelled")
    expect(skill).toContain("reconciled")
    expect(skill).toContain("Never automatically replay")
    expect(skill).toContain("safe to retry")
  })

  test("documents permission model with no dedicated terminal permissions", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain("does not add its own permission category")
    expect(skill).toContain("Bash authorization")
    expect(skill).toContain("unsandboxed")
  })

  test("documents shell quoting contract", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill).toContain('"$PROJECT"')
    expect(skill).toContain('"$CWD"')
    expect(skill).toContain("'don'")
    expect(skill).toContain("'t'")
  })

  test("contains no adapter or migration framing", async () => {
    const skill = await readFile(skillPath, "utf8")
    const lower = skill.toLowerCase()
    expect(lower).not.toContain("migration")
    expect(lower).not.toContain("adapter")
    expect(lower).not.toContain("tool-based integration")
  })

  const legacyDesc = ["has no legacy", "terminal" + "_* tool syntax zellij action or pane-id"].join(
    " ",
  )
  test(legacyDesc, async () => {
    const skill = await readFile(skillPath, "utf8")
    const legacyPrefix = "terminal" + "_"
    for (const verb of ["start", "read", "send", "press", "stop", "list"]) {
      expect(skill).not.toContain(legacyPrefix + verb)
    }
    expect(skill.toLowerCase()).not.toContain("zellij action")
    expect(skill).not.toContain("pane-id")
  })

  test("skill file is under 500 lines", async () => {
    const skill = await readFile(skillPath, "utf8")
    expect(skill.split("\n").length).toBeLessThan(500)
  })

  test("rejects missing CLI example", async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const verb of ["start", "read", "send", "press", "stop", "list"]) {
      const hasExample = skill.includes(`agent-terminal --project "$PROJECT" ${verb}`)
      expect(hasExample).toBe(true)
    }
  })
})

describe("adapter artifacts are absent", () => {
  const d1 = `${["opencode", "tools", "terminal.ts"].join("/")} no longer exists`
  test(d1, async () => {
    const p = resolve(projectRoot, "opencode", "tools", "terminal.ts")
    await expect(access(p)).rejects.toThrow()
  })

  const d2 = `${["opencode", "tests", "terminal.test.ts"].join("/")} no longer exists`
  test(d2, async () => {
    const p = resolve(projectRoot, "opencode", "tests", "terminal.test.ts")
    await expect(access(p)).rejects.toThrow()
  })

  const d3 = `package.json contains no ${["@opencode-ai", "plugin"].join("/")}`
  test(d3, async () => {
    const p = resolve(projectRoot, "opencode/package.json")
    const pkg = await readFile(p, "utf8")
    const forbidden = ["@opencode-ai", "plugin"].join("/")
    expect(pkg).not.toContain(forbidden)
  })

  const d4 = `tsconfig.json does not include ${["tools", "**", "*.ts"].join("/")}`
  test(d4, async () => {
    const p = resolve(projectRoot, "opencode/tsconfig.json")
    const tsconfig = await readFile(p, "utf8")
    const forbidden = ["tools", "**", "*.ts"].join("/")
    expect(tsconfig).not.toContain(forbidden)
  })

  const prefix = "terminal" + "_"
  const d5 = `SKILL.md contains no ${prefix}* tool-call syntax`
  test(d5, async () => {
    const skill = await readFile(skillPath, "utf8")
    for (const verb of ["start", "read", "send", "press", "stop", "list"]) {
      expect(skill).not.toContain(prefix + verb)
    }
  })
})
