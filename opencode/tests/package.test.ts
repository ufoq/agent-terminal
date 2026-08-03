import { existsSync, statSync } from "node:fs"
import { readFile } from "node:fs/promises"
import { dirname, join, relative } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "bun:test"

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
const npmRoot = join(repoRoot, "opencode", "npm")
const packagesDir = join(npmRoot, "packages")

const slimPackage = join(packagesDir, "opencode-agent-terminal")
const bundlePackage = join(packagesDir, "opencode-agent-terminal-bundle-zellij")

const expectedVersion = "0.1.2"

type PackageManifest = {
  readonly name: string
  readonly version: string
  readonly description: string
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
  readonly publishConfig?: { access: string }
}

type SkillConfig = {
  skills?: {
    paths?: string[]
  }
}

type ShellEnvOutput = {
  env: Record<string, string | undefined>
}

type ShellEnvInput = {
  sessionID: string
}

async function readJson<T>(path: string): Promise<T> {
  return JSON.parse(await readFile(path, "utf8")) as T
}

async function importSharedModule() {
  return import("../npm/src/index")
}

describe("npm package contract", () => {
  it("builds both package manifests with the same version", async () => {
    const slim = await readJson<PackageManifest>(join(slimPackage, "package.json"))
    const bundle = await readJson<PackageManifest>(join(bundlePackage, "package.json"))

    expect(slim.name).toBe("@ufoq/opencode-agent-terminal")
    expect(bundle.name).toBe("@ufoq/opencode-agent-terminal-bundle-zellij")
    expect(slim.version).toBe(expectedVersion)
    expect(bundle.version).toBe(expectedVersion)
    expect(slim.version).toBe(bundle.version)

    for (const manifest of [slim, bundle]) {
      expect(manifest.type).toBe("module")
      expect(manifest.main).toBe("dist/index.js")
      expect(manifest.types).toBe("dist/index.d.ts")
      expect(manifest["oc-plugin"]).toEqual(["server"])
      expect(manifest.os).toEqual(["linux"])
      expect(manifest.cpu).toEqual(["x64"])
      expect(manifest.dependencies ?? {}).toEqual({})
      expect(manifest.peerDependencies ?? {}).toEqual({})
      expect(Object.keys(manifest.scripts ?? {})).not.toContain("postinstall")
      expect(manifest.publishConfig).toEqual({ access: "public" })
    }
  })

  it("keeps Zellij out of the slim package files list", async () => {
    const slim = await readJson<PackageManifest>(join(slimPackage, "package.json"))
    expect(slim.files).toEqual(["dist", "skills", "bin/linux-x64", "LICENSE", "README.md"])
  })

  it("includes bundled Zellij in the bundle package files list", async () => {
    const bundle = await readJson<PackageManifest>(join(bundlePackage, "package.json"))
    expect(bundle.files).toEqual([
      "dist",
      "skills",
      "bin/linux-x64",
      "bin/zellij",
      "LICENSE",
      "README.md",
      "THIRD_PARTY.md",
    ])
  })

  it("exports the V1 plugin object from the shared source", async () => {
    const module = await importSharedModule()
    expect(module.default).toEqual({ id: "opencode-agent-terminal", server: module.server })
  })

  it("registers the bundled skills path and agent-terminal binary for the slim package", async () => {
    const module = await importSharedModule()
    const hooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot: slimPackage,
      stderr: () => undefined,
    })

    const config: SkillConfig = {}
    hooks.config?.(config)
    expect(config.skills?.paths).toEqual([join(slimPackage, "skills")])

    const output: ShellEnvOutput = { env: { PATH: "/usr/bin" } }
    hooks["shell.env"]?.({ sessionID: "test-session-a" }, output)
    expect(output.env["PATH"]?.split(":")[0]).toBe(join(slimPackage, "bin", "linux-x64"))
    expect(output.env["AGENT_TERMINAL_SCOPE"]).toBe("test-session-a")
  })

  it("registers the bundled skills path and puts bundled Zellij first for the bundle package", async () => {
    const module = await importSharedModule()
    const hooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot: bundlePackage,
      stderr: () => undefined,
    })

    const config: SkillConfig = {}
    hooks.config?.(config)
    expect(config.skills?.paths).toEqual([join(bundlePackage, "skills")])

    const output: ShellEnvOutput = { env: { PATH: "/usr/bin" } }
    hooks["shell.env"]?.({ sessionID: "test-session-b" }, output)
    expect(output.env["PATH"]?.split(":")[0]).toBe(join(bundlePackage, "bin", "zellij"))
    expect(output.env["PATH"]?.split(":")[1]).toBe(join(bundlePackage, "bin", "linux-x64"))
    expect(output.env["AGENT_TERMINAL_SCOPE"]).toBe("test-session-b")
  })

  it("rejects a missing or empty sessionID to avoid collapsing scopes", async () => {
    const module = await importSharedModule()
    const hooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot: slimPackage,
      stderr: () => undefined,
    })

    const missing = hooks["shell.env"]
    const empty: ShellEnvInput = { sessionID: "  " }
    if (!missing) throw new Error("shell.env hook missing")
    expect(() => missing({ sessionID: "" }, { env: {} })).toThrow(/sessionID/)
    expect(() => missing(empty, { env: {} })).toThrow(/sessionID/)
  })

  it("keeps the agent-terminal binary executable in both packages", () => {
    for (const packageRoot of [slimPackage, bundlePackage]) {
      const binPath = join(packageRoot, "bin", "linux-x64", "agent-terminal")
      const stat = statSync(binPath)
      expect(stat.isFile()).toBe(true)
      expect((stat.mode & 0o111) !== 0).toBe(true)
      expect(relative(packageRoot, binPath)).toBe(join("bin", "linux-x64", "agent-terminal"))
    }
  })

  it("bundles the skill in both packages", () => {
    for (const packageRoot of [slimPackage, bundlePackage]) {
      const skillPath = join(packageRoot, "skills", "agent-terminal", "SKILL.md")
      expect(existsSync(skillPath)).toBe(true)
    }
  })

  it("bundles a pinned x86_64 Zellij binary only in the bundle package", () => {
    const bundleZellij = join(bundlePackage, "bin", "zellij", "zellij")
    const stat = statSync(bundleZellij)
    expect(stat.isFile()).toBe(true)
    expect((stat.mode & 0o111) !== 0).toBe(true)
    expect(relative(bundlePackage, bundleZellij)).toBe(join("bin", "zellij", "zellij"))

    const slimZellij = join(slimPackage, "bin", "zellij", "zellij")
    expect(existsSync(slimZellij)).toBe(false)
  })

  it("withholds hooks when the agent-terminal binary is missing or unsupported", async () => {
    const module = await importSharedModule()
    const diagnostics: string[] = []

    const missingHooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot: join(slimPackage, "missing-root"),
      stderr: (message: string) => diagnostics.push(message),
    })
    const unsupportedHooks = await module.createServerHooks({
      arch: "riscv64",
      platform: "linux",
      packageRoot: slimPackage,
      stderr: (message: string) => diagnostics.push(message),
    })

    expect(missingHooks).toEqual({})
    expect(unsupportedHooks).toEqual({})
    expect(diagnostics.length).toBe(2)
  })

  it("exposes hooks for the slim package even though Zellij is absent", async () => {
    const module = await importSharedModule()
    const hooks = await module.createServerHooks({
      arch: "x64",
      platform: "linux",
      packageRoot: slimPackage,
      stderr: (message: string) => {
        throw new Error(`Unexpected diagnostic: ${message}`)
      },
    })

    expect(Object.keys(hooks)).toEqual(["config", "shell.env"])
  })
})
