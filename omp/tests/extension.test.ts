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

type ToolCallOutcome = {
  readonly effectiveInput: Record<string, unknown> | undefined
  readonly result: ToolCallResult | undefined
}

function invokeToolCall(
  handlers: Map<string, (...args: unknown[]) => unknown>,
  event: { input?: Record<string, unknown>; toolName: string },
  ctx: ToolCallCtx = {},
): ToolCallOutcome {
  const handler = handlers.get("tool_call")
  if (handler === undefined) {
    return { effectiveInput: event.input, result: undefined }
  }
  // Model omp's effective-input semantics: a returned revision replaces the
  // original input; an undefined return leaves the original input untouched.
  const result = handler(event, ctx) as ToolCallResult | undefined
  return { effectiveInput: result?.input ?? event.input, result }
}

function envOf(outcome: ToolCallOutcome): Record<string, unknown> {
  expect(outcome.effectiveInput).toBeTypeOf("object")
  const env = outcome.effectiveInput?.["env"]
  expect(env).toBeTypeOf("object")
  return env as Record<string, unknown>
}

function pathParts(path: string): string[] {
  // Empty components are retained, never filtered: on Linux an empty PATH
  // component means the CWD, so a constructed path ending in ":" (or with any
  // empty component) must not be masked by splitting here.
  return path.split(":")
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
    const event = { toolName: "write_file", input: { path: "/tmp/x" } }
    const outcome = invokeToolCall(handlers, event)
    expect(outcome.result).toBeUndefined()
    // The effective input is unchanged: no revision is applied.
    expect(outcome.effectiveInput).toEqual(event.input)
  })

  it("rejects a null env unrevised", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const event = {
      toolName: "bash",
      input: { command: "echo hi", env: null as never },
    }
    const outcome = invokeToolCall(handlers, event, {
      sessionManager: { getSessionId: () => "sess-123" },
    })
    // Untouched (not even the session scope is injected): the native bash
    // schema validation reports the malformed env. The effective input is the
    // original, not the returned undefined.
    expect(outcome.result).toBeUndefined()
    expect(outcome.effectiveInput).toEqual(event.input)
  })

  it("rejects an array env unrevised", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const event = {
      toolName: "bash",
      input: { command: "echo hi", env: ["PATH=/usr/bin"] as never },
    }
    const outcome = invokeToolCall(handlers, event, {
      sessionManager: { getSessionId: () => "sess-123" },
    })
    expect(outcome.result).toBeUndefined()
    expect(outcome.effectiveInput).toEqual(event.input)
  })

  it("rejects an env with a non-string value unrevised", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const event = {
      toolName: "bash",
      input: {
        command: "echo hi",
        env: { PATH: 123 } as never,
      },
    }
    const outcome = invokeToolCall(handlers, event, {
      sessionManager: { getSessionId: () => "sess-123" },
    })
    // The explicit invalid value is not revised away and no session scope or
    // bundled PATH is injected; the effective input is the original.
    expect(outcome.result).toBeUndefined()
    expect(outcome.effectiveInput).toEqual(event.input)
  })

  it("injects the session id as AGENT_TERMINAL_SCOPE when env is absent", () => {
    const root = createPackageLayout({ withZellij: true })
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const outcome = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(envOf(outcome)["AGENT_TERMINAL_SCOPE"]).toBe("sess-123")
    expect(outcome.result?.input?.["command"]).toBe("echo hi")
  })

  it("preserves an explicit AGENT_TERMINAL_SCOPE", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const outcome = invokeToolCall(
      handlers,
      {
        toolName: "bash",
        input: { command: "echo hi", env: { AGENT_TERMINAL_SCOPE: "shared" } },
      },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(envOf(outcome)["AGENT_TERMINAL_SCOPE"]).toBe("shared")
  })

  it("treats empty or whitespace AGENT_TERMINAL_SCOPE as absent", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    for (const emptyScope of ["", "   "]) {
      const outcome = invokeToolCall(
        handlers,
        {
          toolName: "bash",
          input: { command: "echo hi", env: { AGENT_TERMINAL_SCOPE: emptyScope } },
        },
        { sessionManager: { getSessionId: () => "sess-123" } },
      )
      expect(envOf(outcome)["AGENT_TERMINAL_SCOPE"]).toBe("sess-123")
    }
  })

  it("preserves other env keys and passes command/timeout/cwd through unchanged", () => {
    const root = createPackageLayout()
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const outcome = invokeToolCall(
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
    const env = envOf(outcome)
    expect(env["FOO"]).toBe("bar")
    expect(env["HOME"]).toBe("/home/x")
    expect(env["AGENT_TERMINAL_SCOPE"]).toBe("sess-123")
    expect(outcome.result?.input?.["command"]).toBe("make test")
    expect(outcome.result?.input?.["timeout"]).toBe(42)
    expect(outcome.result?.input?.["cwd"]).toBe("/tmp/project")
  })

  it("falls back to process PATH and keeps host entries when env.PATH is absent", () => {
    const root = createPackageLayout({ withZellij: true })
    process.env["PATH"] = "/usr/local/bin:/usr/bin"
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const outcome = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    // Compare the constructed PATH value exactly: pathParts retains empty
    // components, so a trailing ":" (CWD) cannot slip through.
    const parts = pathParts(envOf(outcome)["PATH"] as string)
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
    const outcome = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi", env: { PATH: "" } } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    // `??` (not `||`): an explicit empty PATH stays the base, so no process
    // PATH entry leaks in. Compare the constructed value exactly.
    const parts = pathParts(envOf(outcome)["PATH"] as string)
    expect(parts).toEqual([join(root, "bin", "zellij"), join(root, "bin", "linux-x64")])
    expect(parts).not.toContain("")
  })

  it("prepends bundled dirs to a supplied env.PATH without duplicates", () => {
    const root = createPackageLayout({ withZellij: true })
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const outcome = invokeToolCall(
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
    const parts = pathParts(envOf(outcome)["PATH"] as string)
    expect(parts).toEqual([join(root, "bin", "zellij"), join(root, "bin", "linux-x64"), "/opt/bin"])
  })

  it("skips the zellij bin dir when the bundled zellij is absent", () => {
    const root = createPackageLayout()
    process.env["PATH"] = "/usr/bin"
    const { api, handlers } = createFakeApi()
    loadExtension(api, { packageRoot: root, platform: "linux", arch: "x64" })
    const outcome = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const parts = pathParts(envOf(outcome)["PATH"] as string)
    expect(parts).toEqual([join(root, "bin", "linux-x64"), "/usr/bin"])
  })

  it("handles every unavailable sessionManager shape with one diagnostic per extension", () => {
    const root = createPackageLayout()
    const shapes: Array<{ name: string; ctx: ToolCallCtx }> = [
      { name: "missing", ctx: {} },
      { name: "empty-id", ctx: { sessionManager: { getSessionId: () => "" } } },
      {
        name: "non-function",
        ctx: { sessionManager: { getSessionId: "not-a-function" as never } },
      },
    ]
    for (const shape of shapes) {
      // Fresh extension per shape: each carries its own warning-dedup state.
      const diagnostics: string[] = []
      const { api, handlers } = createFakeApi()
      loadExtension(api, {
        packageRoot: root,
        platform: "linux",
        arch: "x64",
        stderr: (message) => diagnostics.push(message),
      })
      for (let call = 0; call < 2; call++) {
        expect(() =>
          invokeToolCall(handlers, { toolName: "bash", input: { command: "echo hi" } }, shape.ctx),
        ).not.toThrow()
        const outcome = invokeToolCall(
          handlers,
          { toolName: "bash", input: { command: "echo hi" } },
          shape.ctx,
        )
        const env = envOf(outcome)
        // No scope injection, but the bundled PATH is still handled.
        expect(env["AGENT_TERMINAL_SCOPE"]).toBeUndefined()
        expect(pathParts(env["PATH"] as string)[0]).toBe(join(root, "bin", "linux-x64"))
      }
      // Exactly one diagnostic per extension, emitted on the first call only.
      expect(diagnostics).toHaveLength(1)
      expect(diagnostics[0]).toContain("no session id available from omp")
    }
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
    // A working extension means isExecutableFile (test -f and test -x)
    // accepted the bundled binaries; the PATH revision also proves the
    // tool_call handler is live.
    const outcome = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    expect(pathParts(envOf(outcome)["PATH"] as string)[0]).toBe(join(root, "bin", "zellij"))
  })

  it("executable guard rejects a directory even when it is executable", () => {
    const root = mkdtempSync(join(tmpdir(), "omp-agent-terminal-dir-"))
    tempRoots.push(root)
    const binDir = join(root, "bin", "linux-x64")
    mkdirSync(binDir, { recursive: true, mode: 0o755 })
    // The "binary" is a directory with execute bits: `test -x` alone would
    // pass, but the regular-file requirement must reject it.
    mkdirSync(join(binDir, "agent-terminal"), { mode: 0o755 })
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
    const outcome = invokeToolCall(
      handlers,
      { toolName: "bash", input: { command: "echo hi" } },
      { sessionManager: { getSessionId: () => "sess-123" } },
    )
    const parts = pathParts(envOf(outcome)["PATH"] as string)
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
