import { existsSync, statSync } from "node:fs"
import { readFile } from "node:fs/promises"
import { dirname, join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "bun:test"

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
const packageRoot = join(repoRoot, "opencode", "npm", "opencode-agent-terminal")

type PackageManifest = {
  readonly name: string
  readonly version: string
  readonly type: string
  readonly main: string
  readonly types: string
  readonly files: readonly string[]
  readonly os: readonly string[]
  readonly cpu: readonly string[]
  readonly scripts?: Record<string, string>
  readonly dependencies?: Record<string, string>
  readonly peerDependencies?: Record<string, string>
  readonly devDependencies?: Record<string, string>
  readonly "oc-plugin": readonly string[]
}

type SkillConfig = {
  skills?: {
    paths?: string[]
  }
}

type ShellEnvOutput = {
  env: Record<string, string | undefined>
}

async function readJson<T>(path: string): Promise<T> {
  return JSON.parse(await readFile(path, "utf8")) as T
}

describe("npm package contract", () => {
  it("declares the public OpenCode plugin package metadata", async () => {
    const manifest = await readJson<PackageManifest>(join(packageRoot, "package.json"))

    expect(manifest.name).toBe("@ufoq/opencode-agent-terminal")
    expect(manifest.version).toBe("0.1.0")
    expect(manifest.type).toBe("module")
    expect(manifest.main).toBe("dist/index.js")
    expect(manifest.types).toBe("dist/index.d.ts")
    expect(manifest["oc-plugin"]).toEqual(["server"])
    expect(manifest.os).toEqual(["linux"])
    expect(manifest.cpu).toEqual(["x64"])
    expect(manifest.files).toEqual(["dist", "skills", "bin", "LICENSE", "README.md"])
    expect(manifest.dependencies ?? {}).toEqual({})
    expect(manifest.peerDependencies ?? {}).toEqual({})
    expect(Object.keys(manifest.scripts ?? {})).not.toContain("postinstall")
  })

  it("bundles the agent-terminal skill at the path registered by the plugin", () => {
    const skillPath = join(packageRoot, "skills", "agent-terminal", "SKILL.md")
    expect(existsSync(skillPath)).toBe(true)
  })

  it("registers the bundled skills path and selected binary path", async () => {
    const module = await import("../npm/opencode-agent-terminal/src/index")
    expect(module.default).toEqual({ id: "opencode-agent-terminal", server: module.server })
    const hooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot,
      stderr: () => undefined,
    })

    const config: SkillConfig = {}
    hooks.config?.(config)

    expect(config.skills?.paths).toEqual([join(packageRoot, "skills")])

    const output: ShellEnvOutput = { env: { PATH: "/usr/bin" } }
    hooks["shell.env"]?.({}, output)

    expect(output.env["PATH"]?.split(":")[0]).toBe(join(packageRoot, "bin", "linux-x64"))
  })

  it("withholds hooks when the selected binary is missing or unsupported", async () => {
    const module = await import("../npm/opencode-agent-terminal/src/index")
    const diagnostics: string[] = []

    const missingHooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot: join(packageRoot, "missing-root"),
      stderr: (message: string) => diagnostics.push(message),
    })
    const unsupportedHooks = await module.createServerHooks({
      arch: "riscv64",
      platform: "linux",
      packageRoot,
      stderr: (message: string) => diagnostics.push(message),
    })

    expect(missingHooks).toEqual({})
    expect(unsupportedHooks).toEqual({})
    expect(diagnostics.length).toBe(2)
  })

  it("keeps the packaged x86_64 binary artifact executable", () => {
    const binPath = join(packageRoot, "bin", "linux-x64", "agent-terminal")
    const stat = statSync(binPath)
    expect(stat.isFile()).toBe(true)
    expect((stat.mode & 0o111) !== 0).toBe(true)
    expect(relative(packageRoot, binPath)).toBe(join("bin", "linux-x64", "agent-terminal"))
  })
})
