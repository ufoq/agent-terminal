import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { afterEach, beforeEach, describe, expect, it } from "bun:test"
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"

import { createExtension } from "../npm/src/index"

type FlagRegistration = {
  name: string
  options: { description?: string; type: "boolean" | "string"; default?: boolean | string }
}

type FakeApi = {
  registerFlag(name: string, options: FlagRegistration["options"]): void
  on(event: string, handler: (...args: unknown[]) => unknown): void
  registerTool(tool: unknown): void
}

type FakeApiHarness = {
  api: FakeApi
  flags: FlagRegistration[]
  handlers: Map<string, (...args: unknown[]) => unknown>
  tools: unknown[]
}

function createFakeApi(): FakeApiHarness {
  const flags: FlagRegistration[] = []
  const tools: unknown[] = []
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  return {
    flags,
    tools,
    handlers,
    api: {
      registerFlag(name: string, options: FlagRegistration["options"]): void {
        flags.push({ name, options })
      },
      on(event: string, handler: (...args: unknown[]) => unknown): void {
        handlers.set(event, handler)
      },
      registerTool(tool: unknown): void {
        tools.push(tool)
      },
    },
  }
}

const tempRoots: string[] = []

type PackageLayoutOptions = {
  readonly withZellij?: boolean
}

function createPackageLayout(options: PackageLayoutOptions = {}): string {
  const root = mkdtempSync(join(tmpdir(), "pi-agent-terminal-test-"))
  tempRoots.push(root)
  const binDir = join(root, "bin", "linux-x64")
  mkdirSync(binDir, { recursive: true })
  const agentTerminalBin = join(binDir, "agent-terminal")
  writeFileSync(agentTerminalBin, "#!/bin/sh\nexit 0\n")
  chmodSync(agentTerminalBin, 0o755)
  if (options.withZellij === true) {
    const zellijDir = join(root, "bin", "zellij")
    mkdirSync(zellijDir, { recursive: true })
    const zellijBin = join(zellijDir, "zellij")
    writeFileSync(zellijBin, "#!/bin/sh\nexit 0\n")
    chmodSync(zellijBin, 0o755)
  }
  mkdirSync(join(root, "skills", "agent-terminal"), { recursive: true })
  return root
}

function createEmptyRoot(): string {
  const root = mkdtempSync(join(tmpdir(), "pi-agent-terminal-empty-"))
  tempRoots.push(root)
  return root
}

function createManagedDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "pi-agent-terminal-agentdir-"))
  tempRoots.push(dir)
  return dir
}

type CreateExtensionTestInput = {
  readonly arch?: string
  readonly packageRoot: string
  readonly platform?: string
  readonly stderr?: (message: string) => void
}

type PiManifest = {
  readonly pi: {
    readonly extensions: readonly string[]
    readonly skills: readonly string[]
  }
}

type ReleaseManifest = {
  readonly version: string
}

type PackageNameVersionManifest = {
  readonly name: string
  readonly version: string
}

function loadExtension(api: FakeApi, input: CreateExtensionTestInput): void {
  createExtension(input)(api as unknown as ExtensionAPI)
}

function pathParts(): string[] {
  return (process.env["PATH"] ?? "").split(":").filter((entry) => entry !== "")
}

const ORIGINAL_PATH = process.env["PATH"] ?? ""

describe("pi agent-terminal extension", () => {
  beforeEach(() => {
    process.env["PATH"] = ORIGINAL_PATH
  })

  afterEach(() => {
    process.env["PATH"] = ORIGINAL_PATH
    delete process.env["PI_CODING_AGENT_DIR"]
    for (const root of tempRoots) {
      rmSync(root, { recursive: true, force: true })
    }
    tempRoots.length = 0
  })

  it("never registers a tool (stock bash stays untouched)", () => {
    const root = createPackageLayout()
    const { api, tools } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(tools).toEqual([])
  })

  it("registers exactly the cwd and no-lsp compatibility flags", () => {
    const root = createPackageLayout()
    const { api, flags } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(flags.map((flag) => flag.name)).toEqual(["cwd", "no-lsp"])
    expect(flags.map((flag) => flag.options.type)).toEqual(["string", "boolean"])
    for (const flag of flags) {
      expect(typeof flag.options.description).toBe("string")
    }
  })

  it("orders PATH as bundled dirs, managed bin, then the rest, with the managed bin exactly once", () => {
    const root = createPackageLayout({ withZellij: true })
    const agentDir = createManagedDir()
    process.env["PI_CODING_AGENT_DIR"] = agentDir
    process.env["PATH"] = "/usr/local/bin:/usr/bin"
    const { api } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(pathParts()).toEqual([
      join(root, "bin", "zellij"),
      join(root, "bin", "linux-x64"),
      join(agentDir, "bin"),
      "/usr/local/bin",
      "/usr/bin",
    ])
    const managedEntries = pathParts().filter((entry) => entry === join(agentDir, "bin"))
    expect(managedEntries).toHaveLength(1)
  })

  it("prepends the zellij bin dir when the bundled zellij exists", () => {
    const root = createPackageLayout({ withZellij: true })
    const agentDir = createManagedDir()
    process.env["PI_CODING_AGENT_DIR"] = agentDir
    process.env["PATH"] = "/usr/bin"
    const { api } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(pathParts()[0]).toBe(join(root, "bin", "zellij"))
    expect(pathParts()[1]).toBe(join(root, "bin", "linux-x64"))
  })

  it("skips the zellij bin dir when the bundled zellij is absent", () => {
    const root = createPackageLayout()
    const agentDir = createManagedDir()
    process.env["PI_CODING_AGENT_DIR"] = agentDir
    process.env["PATH"] = "/usr/bin"
    const { api } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(pathParts()[0]).toBe(join(root, "bin", "linux-x64"))
    expect(pathParts()).not.toContain(join(root, "bin", "zellij"))
  })

  it("does not duplicate PATH entries that are already present", () => {
    const root = createPackageLayout({ withZellij: true })
    const agentDir = createManagedDir()
    process.env["PI_CODING_AGENT_DIR"] = agentDir
    process.env["PATH"] = `${join(agentDir, "bin")}:${join(root, "bin", "linux-x64")}:/usr/bin`
    const { api } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const parts = pathParts()
    expect(parts.filter((entry) => entry === join(agentDir, "bin"))).toHaveLength(1)
    expect(parts.filter((entry) => entry === join(root, "bin", "linux-x64"))).toHaveLength(1)
    expect(parts.filter((entry) => entry === join(root, "bin", "zellij"))).toHaveLength(1)
    expect(parts.slice(3)).toEqual(["/usr/bin"])
  })

  it("registers no event handlers after a successful load", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(handlers.size).toBe(0)
  })

  it("does nothing on non-linux platforms and reports a diagnostic", () => {
    const root = createPackageLayout()
    process.env["PATH"] = "/usr/bin"
    const diagnostics: string[] = []
    const { api, flags, tools } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "darwin",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(process.env["PATH"]).toBe("/usr/bin")
    expect(diagnostics).toEqual([
      "[agent-terminal] unsupported platform: darwin. This package supports Linux only.",
    ])
    expect(flags).toEqual([])
    expect(tools).toEqual([])
  })

  it("does nothing on non-x64 architectures and reports a diagnostic", () => {
    const root = createPackageLayout()
    process.env["PATH"] = "/usr/bin"
    const diagnostics: string[] = []
    const { api } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "arm64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(process.env["PATH"]).toBe("/usr/bin")
    expect(diagnostics).toEqual([
      "[agent-terminal] unsupported architecture: arm64. This package supports x86_64 Linux only.",
    ])
  })

  it("reports a diagnostic and skips PATH mutation when the bundled binary is missing", () => {
    const root = createEmptyRoot()
    process.env["PATH"] = "/usr/bin"
    const diagnostics: string[] = []
    const { api, flags } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(process.env["PATH"]).toBe("/usr/bin")
    expect(diagnostics).toEqual([
      `[agent-terminal] bundled executable is missing or not executable: ${join(root, "bin", "linux-x64", "agent-terminal")}`,
    ])
    // The compatibility flags are still registered so the CLI accepts them.
    expect(flags.map((flag) => flag.name)).toEqual(["cwd", "no-lsp"])
  })

  it("derives the PATH entries from the injected packageRoot", () => {
    const root = createPackageLayout({ withZellij: true })
    process.env["PATH"] = "/usr/bin"
    const { api } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const parts = pathParts()
    expect(parts[0]).toBe(join(root, "bin", "zellij"))
    expect(parts[1]).toBe(join(root, "bin", "linux-x64"))
    expect(parts.slice(3)).toEqual(["/usr/bin"])
  })

  it("declares the extension and skill resources in both package manifests", () => {
    const testDir = dirname(fileURLToPath(import.meta.url))
    const packageRoot = join(testDir, "..", "npm", "packages")
    const manifests = [
      join(packageRoot, "pi-agent-terminal", "package.json"),
      join(packageRoot, "pi-agent-terminal-bundle-zellij", "package.json"),
    ]
    for (const manifestPath of manifests) {
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8")) as PiManifest
      expect(manifest.pi.extensions).toEqual(["./dist/index.js"])
      expect(manifest.pi.skills).toEqual(["./skills"])
    }
  })

  it("keeps every package manifest name and version aligned with the canonical release version", () => {
    const testDir = dirname(fileURLToPath(import.meta.url))
    const repoRoot = join(testDir, "..", "..")
    const release = JSON.parse(
      readFileSync(join(repoRoot, "release.json"), "utf8"),
    ) as ReleaseManifest
    const packages: readonly { readonly path: string; readonly name: string }[] = [
      {
        path: join(repoRoot, "pi", "npm", "packages", "pi-agent-terminal", "package.json"),
        name: "@ufoq/pi-agent-terminal",
      },
      {
        path: join(
          repoRoot,
          "pi",
          "npm",
          "packages",
          "pi-agent-terminal-bundle-zellij",
          "package.json",
        ),
        name: "@ufoq/pi-agent-terminal-bundle-zellij",
      },
      {
        path: join(repoRoot, "omp", "npm", "packages", "omp-agent-terminal", "package.json"),
        name: "@ufoq/omp-agent-terminal",
      },
      {
        path: join(
          repoRoot,
          "omp",
          "npm",
          "packages",
          "omp-agent-terminal-bundle-zellij",
          "package.json",
        ),
        name: "@ufoq/omp-agent-terminal-bundle-zellij",
      },
      {
        path: join(
          repoRoot,
          "opencode",
          "npm",
          "packages",
          "opencode-agent-terminal",
          "package.json",
        ),
        name: "@ufoq/opencode-agent-terminal",
      },
      {
        path: join(
          repoRoot,
          "opencode",
          "npm",
          "packages",
          "opencode-agent-terminal-bundle-zellij",
          "package.json",
        ),
        name: "@ufoq/opencode-agent-terminal-bundle-zellij",
      },
    ]
    expect(typeof release.version).toBe("string")
    for (const { path, name } of packages) {
      const manifest = JSON.parse(readFileSync(path, "utf8")) as PackageNameVersionManifest
      expect(manifest.name).toBe(name)
      expect(manifest.version).toBe(release.version)
    }
  })
})
