import { afterEach, beforeEach, describe, expect, it } from "bun:test"
import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { createExtension, decodePackageRoot, type ExtensionApi } from "../npm/src/index"

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
  onCalls: string[]
  tools: unknown[]
}

function createFakeApi(): FakeApiHarness {
  const flags: FlagRegistration[] = []
  const tools: unknown[] = []
  const onCalls: string[] = []
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  return {
    flags,
    tools,
    onCalls,
    handlers,
    api: {
      registerFlag(name: string, options: FlagRegistration["options"]): void {
        flags.push({ name, options })
      },
      on(event: string, handler: (...args: unknown[]) => unknown): void {
        onCalls.push(event)
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
  readonly withSpaceInName?: boolean
  readonly withZellij?: boolean
}

function createPackageLayout(options: PackageLayoutOptions = {}): string {
  const prefix =
    options.withSpaceInName === true ? "omp agent terminal test-" : "omp-agent-terminal-test-"
  const root = mkdtempSync(join(tmpdir(), prefix))
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
  const root = mkdtempSync(join(tmpdir(), "omp-agent-terminal-empty-"))
  tempRoots.push(root)
  return root
}

type CreateExtensionTestInput = {
  readonly arch?: string
  readonly packageRoot: string
  readonly platform?: string
  readonly stderr?: (message: string) => void
}

function loadExtension(api: FakeApi, input: CreateExtensionTestInput): void {
  createExtension(input)(api as unknown as ExtensionApi)
}

type FakeSessionManager = {
  getSessionId: () => string | undefined
}

type ToolCallCtx = {
  readonly sessionManager?: FakeSessionManager
}

type ToolCallResult = {
  readonly block?: boolean
  readonly input?: Record<string, unknown>
  readonly reason?: string
}

function invokeToolCall(
  handlers: Map<string, (...args: unknown[]) => unknown>,
  event: { input?: Record<string, unknown>; toolName: string },
  ctx: ToolCallCtx = {},
): ToolCallResult | undefined {
  const handler = handlers.get("tool_call")
  if (handler === undefined) return undefined
  return handler(event, ctx) as ToolCallResult | undefined
}

function envOf(result: ToolCallResult | undefined): Record<string, unknown> {
  expect(result?.input).toBeTypeOf("object")
  const env = result?.input?.["env"]
  expect(env).toBeTypeOf("object")
  return env as Record<string, unknown>
}

function pathParts(path: string): string[] {
  return path.split(":").filter((entry) => entry !== "")
}

const ORIGINAL_PATH = process.env["PATH"] ?? ""

describe("omp agent-terminal extension", () => {
  beforeEach(() => {
    process.env["PATH"] = ORIGINAL_PATH
  })

  afterEach(() => {
    process.env["PATH"] = ORIGINAL_PATH
    for (const root of tempRoots) {
      rmSync(root, { recursive: true, force: true })
    }
    tempRoots.length = 0
  })

  it("never registers a tool (native bash stays untouched)", () => {
    const root = createPackageLayout()
    const { api, tools } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(tools).toEqual([])
  })

  it("registers exactly one tool_call handler and the no-context-files flag", () => {
    const root = createPackageLayout()
    const { api, flags, handlers, onCalls } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    expect(flags.map((flag) => flag.name)).toEqual(["no-context-files"])
    expect(flags.map((flag) => flag.options.type)).toEqual(["boolean"])
    expect(typeof flags[0]?.options.description).toBe("string")
    // Every on() call is recorded, so this is a true registration count.
    expect(onCalls).toEqual(["tool_call"])
    expect(onCalls.filter((event) => event === "tool_call")).toHaveLength(1)
    expect(handlers.has("tool_call")).toBe(true)
  })

  it("leaves non-bash tool calls untouched", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(handlers, {
      toolName: "write_file",
      input: { path: "/tmp/x" },
    })
    expect(result).toBeUndefined()
  })

  it("rejects a null env unrevised", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: { command: "echo hi", env: null as never },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    // Untouched (not even the session scope is injected): the native bash
    // schema validation reports the malformed env.
    expect(result).toBeUndefined()
  })

  it("rejects an array env unrevised", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: { command: "echo hi", env: ["PATH=/usr/bin"] as never },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(result).toBeUndefined()
  })

  it("rejects an env with a non-string value unrevised", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: {
          command: "echo hi",
          env: { PATH: 123 } as never,
        },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    // The explicit invalid value is not revised away and no session scope or
    // bundled PATH is injected.
    expect(result).toBeUndefined()
  })

  it("injects the session id as AGENT_TERMINAL_SCOPE when env is absent", () => {
    const root = createPackageLayout({ withZellij: true })
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(envOf(result)["AGENT_TERMINAL_SCOPE"]).toBe("sess-123")
    expect(result?.input?.["command"]).toBe("echo hi")
  })

  it("preserves an explicit AGENT_TERMINAL_SCOPE", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: { command: "echo hi", env: { AGENT_TERMINAL_SCOPE: "shared" } },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(envOf(result)["AGENT_TERMINAL_SCOPE"]).toBe("shared")
  })

  it("treats empty or whitespace AGENT_TERMINAL_SCOPE as absent", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    for (const emptyScope of ["", "   "]) {
      const result = invokeToolCall(
        handlers,
        {
          toolName: "bash",
          input: { command: "echo hi", env: { AGENT_TERMINAL_SCOPE: emptyScope } },
        },
        { sessionManager: { getSessionId: () => "sess-123" } },
      )
      expect(envOf(result)["AGENT_TERMINAL_SCOPE"]).toBe("sess-123")
    }
  })

  it("preserves other env keys and passes command/timeout/cwd through unchanged", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: {
          command: "make test",
          timeout: 42,
          cwd: "/tmp/project",
          env: { FOO: "bar", HOME: "/home/x" },
        },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const env = envOf(result)
    expect(env["FOO"]).toBe("bar")
    expect(env["HOME"]).toBe("/home/x")
    expect(env["AGENT_TERMINAL_SCOPE"]).toBe("sess-123")
    expect(result?.input?.["command"]).toBe("make test")
    expect(result?.input?.["timeout"]).toBe(42)
    expect(result?.input?.["cwd"]).toBe("/tmp/project")
  })

  it("falls back to process PATH and keeps host entries when env.PATH is absent", () => {
    const root = createPackageLayout({ withZellij: true })
    process.env["PATH"] = "/usr/local/bin:/usr/bin"
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const parts = pathParts(envOf(result)["PATH"] as string)
    expect(parts).toEqual([
      join(root, "bin", "zellij"),
      join(root, "bin", "linux-x64"),
      "/usr/local/bin",
      "/usr/bin",
    ])
  })

  it("preserves an explicit empty env.PATH (bundled dirs only, no process PATH leak)", () => {
    const root = createPackageLayout({ withZellij: true })
    process.env["PATH"] = "/usr/local/bin:/usr/bin"
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi", env: { PATH: "" } } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    // `??` (not `||`): an explicit empty PATH stays the base, so no process
    // PATH entry leaks in.
    const parts = pathParts(envOf(result)["PATH"] as string)
    expect(parts).toEqual([join(root, "bin", "zellij"), join(root, "bin", "linux-x64")])
  })

  it("prepends bundled dirs to a supplied env.PATH without duplicates", () => {
    const root = createPackageLayout({ withZellij: true })
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: {
          command: "echo hi",
          env: { PATH: `${join(root, "bin", "linux-x64")}:/opt/bin` },
        },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const parts = pathParts(envOf(result)["PATH"] as string)
    expect(parts).toEqual([join(root, "bin", "zellij"), join(root, "bin", "linux-x64"), "/opt/bin"])
  })

  it("skips the zellij bin dir when the bundled zellij is absent", () => {
    const root = createPackageLayout()
    process.env["PATH"] = "/usr/bin"
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const parts = pathParts(envOf(result)["PATH"] as string)
    expect(parts).toEqual([join(root, "bin", "linux-x64"), "/usr/bin"])
  })

  it("does not throw when sessionManager is missing, leaves scope unset, and still handles PATH", () => {
    const root = createPackageLayout()
    const diagnostics: string[] = []
    const { api, handlers } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    const result = invokeToolCall(handlers, { toolName: "bash", input: { command: "echo hi" } })
    const env = envOf(result)
    expect(env["AGENT_TERMINAL_SCOPE"]).toBeUndefined()
    expect(pathParts(env["PATH"] as string)[0]).toBe(join(root, "bin", "linux-x64"))
    expect(diagnostics).toHaveLength(1)
    expect(diagnostics[0]).toContain("no session id available from omp")
    // The warning is emitted once, not per call.
    invokeToolCall(handlers, { toolName: "bash", input: { command: "echo hi" } })
    expect(diagnostics).toHaveLength(1)
  })

  it("treats a non-function or empty getSessionId as unavailable", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const emptyResult = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "" } },
    )
    expect(envOf(emptyResult)["AGENT_TERMINAL_SCOPE"]).toBeUndefined()
    const nonFunctionResult = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: "not-a-function" as never } },
    )
    expect(envOf(nonFunctionResult)["AGENT_TERMINAL_SCOPE"]).toBeUndefined()
  })

  it("does nothing on non-linux platforms and reports a diagnostic", () => {
    const root = createPackageLayout()
    const diagnostics: string[] = []
    const { api, flags, handlers, tools } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "darwin",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(diagnostics).toEqual([
      "[agent-terminal] unsupported platform: darwin. This package supports Linux only.",
    ])
    expect(flags).toEqual([])
    expect(handlers.size).toBe(0)
    expect(tools).toEqual([])
  })

  it("does nothing on non-x64 architectures and reports a diagnostic", () => {
    const root = createPackageLayout()
    const diagnostics: string[] = []
    const { api, handlers } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "arm64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(diagnostics).toEqual([
      "[agent-terminal] unsupported architecture: arm64. This package supports x86_64 Linux only.",
    ])
    expect(handlers.size).toBe(0)
  })

  it("reports a diagnostic and registers no handler when the bundled binary is missing", () => {
    const root = createEmptyRoot()
    const diagnostics: string[] = []
    const { api, flags, handlers } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(diagnostics).toEqual([
      `[agent-terminal] bundled executable is missing or not executable: ${join(root, "bin", "linux-x64", "agent-terminal")}`,
    ])
    // The compatibility flag is still registered so the CLI accepts it.
    expect(flags.map((flag) => flag.name)).toEqual(["no-context-files"])
    expect(handlers.size).toBe(0)
  })

  it("executable guard detects a present fake binary via the import-free mechanism", () => {
    const root = createPackageLayout({ withZellij: true })
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    // A working extension means isExecutableFile (Bun.spawnSync test -x)
    // accepted the bundled binaries; the PATH revision also proves the
    // tool_call handler is live.
    const result = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(pathParts(envOf(result)["PATH"] as string)[0]).toBe(join(root, "bin", "zellij"))
  })

  it("executable guard detects a non-executable binary and refuses to load", () => {
    const root = mkdtempSync(join(tmpdir(), "omp-agent-terminal-notexec-"))
    tempRoots.push(root)
    const binDir = join(root, "bin", "linux-x64")
    mkdirSync(binDir, { recursive: true })
    // Mode 0o644: present file but no execute bits.
    writeFileSync(join(binDir, "agent-terminal"), "#!/bin/sh\nexit 0\n")
    const diagnostics: string[] = []
    const { api, handlers } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(diagnostics).toEqual([
      `[agent-terminal] bundled executable is missing or not executable: ${join(binDir, "agent-terminal")}`,
    ])
    expect(handlers.size).toBe(0)
  })

  it("executable guard detects a missing binary and refuses to load", () => {
    const root = createPackageLayout()
    rmSync(join(root, "bin", "linux-x64", "agent-terminal"))
    const diagnostics: string[] = []
    const { api, handlers } = createFakeApi()
    loadExtension(api, {
      packageRoot: root,
      platform: "linux",
      arch: "x64",
      stderr: (message) => diagnostics.push(message),
    })
    expect(diagnostics).toEqual([
      `[agent-terminal] bundled executable is missing or not executable: ${join(root, "bin", "linux-x64", "agent-terminal")}`,
    ])
    expect(handlers.size).toBe(0)
  })

  it("resolves a package root with a space in the path via the DI injection", () => {
    // mkdtempSync substitutes "XXXXXX" with random chars (no spaces), so the
    // prefix's space is the only one in the path.
    const root = createPackageLayout({ withSpaceInName: true })
    process.env["PATH"] = "/usr/bin"
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const result = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const parts = pathParts(envOf(result)["PATH"] as string)
    // The literal space survives and the bundled binary dir is still found
    // ahead of the process PATH.
    expect(parts).toEqual([join(root, "bin", "linux-x64"), "/usr/bin"])
    expect(parts[0]).toContain(" ")
  })

  it("decodePackageRoot decodes percent-encoded path components", () => {
    const root = createPackageLayout({ withSpaceInName: true })
    const url = new URL("../", new URL(`file://${root}/dist/index.js`))
    // The URL pathname of a space-containing path is percent-encoded.
    expect(url.pathname).toContain("%20")
    expect(decodePackageRoot(url)).toBe(root)
  })

  it("decodePackageRoot falls back to the raw pathname on malformed encoding", () => {
    const encoded = "/tmp/broken%ZZ/root"
    const url = new URL("../", new URL(`file://${encoded}/dist/index.js`))
    expect(() => decodeURIComponent(url.pathname)).toThrow()
    expect(decodePackageRoot(url)).toBe("/tmp/broken%ZZ/root")
  })
})
