import { describe, expect, it } from "bun:test"

import { createSpawnHook, deriveSessionId } from "../npm/src/index"

describe("deriveSessionId", () => {
  it("returns the explicit AGENT_TERMINAL_SCOPE when set", () => {
    const id = deriveSessionId({ AGENT_TERMINAL_SCOPE: "explicit-scope" })
    expect(id).toBe("explicit-scope")
  })

  it("returns PI_SESSION_ID when scope is unset", () => {
    const id = deriveSessionId({ PI_SESSION_ID: "01a00546-58fa-7000-9ae9-99661b480d76" })
    expect(id).toBe("01a00546-58fa-7000-9ae9-99661b480d76")
  })

  it("honors AGENT_TERMINAL_SCOPE over PI_SESSION_ID", () => {
    const id = deriveSessionId({
      AGENT_TERMINAL_SCOPE: "explicit",
      PI_SESSION_ID: "pi-session",
    })
    expect(id).toBe("explicit")
  })

  it("derives session id from PI_SESSION_FILE basename", () => {
    const id = deriveSessionId({
      PI_SESSION_FILE:
        "/home/user/.omp/agent/sessions/-work/2026-08-15T11-54-51-514Z_01a00546-58fa-7000-9ae9-99661b480d76.jsonl",
    })
    expect(id).toBe("01a00546-58fa-7000-9ae9-99661b480d76")
  })

  it("derives session id from PI_SESSION_FILE with simple filename", () => {
    const id = deriveSessionId({ PI_SESSION_FILE: "/tmp/sessions/abc123.jsonl" })
    expect(id).toBe("abc123")
  })

  it("returns null when both PI_SESSION_ID and PI_SESSION_FILE are unset", () => {
    const id = deriveSessionId({})
    expect(id).toBeNull()
  })

  it("treats empty AGENT_TERMINAL_SCOPE as absent", () => {
    const id = deriveSessionId({ AGENT_TERMINAL_SCOPE: "", PI_SESSION_ID: "fallback" })
    expect(id).toBe("fallback")
  })

  it("treats whitespace AGENT_TERMINAL_SCOPE as absent", () => {
    const id = deriveSessionId({ AGENT_TERMINAL_SCOPE: "   ", PI_SESSION_ID: "fallback" })
    expect(id).toBe("fallback")
  })

  it("treats empty PI_SESSION_ID as absent and falls through to PI_SESSION_FILE", () => {
    const id = deriveSessionId({
      PI_SESSION_ID: "",
      PI_SESSION_FILE: "/tmp/123_abc-def.jsonl",
    })
    expect(id).toBe("abc-def")
  })

  it("returns null when PI_SESSION_FILE basename has no underscore-separated suffix", () => {
    const id = deriveSessionId({ PI_SESSION_FILE: "/tmp/.jsonl" })
    expect(id).toBeNull()
  })

  it("trims the derived session id", () => {
    const id = deriveSessionId({ PI_SESSION_ID: "  session-with-spaces  " })
    expect(id).toBe("session-with-spaces")
  })
})

describe("createSpawnHook", () => {
  it("injects AGENT_TERMINAL_SCOPE from PI_SESSION_ID", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: { PI_SESSION_ID: "test-session-123", PATH: "/usr/bin" },
    })
    expect(result.env["AGENT_TERMINAL_SCOPE"]).toBe("test-session-123")
  })

  it("injects AGENT_TERMINAL_SCOPE from PI_SESSION_FILE", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: {
        PI_SESSION_FILE: "/tmp/2026-01-01T00-00-00-000Z_abc-def-123.jsonl",
        PATH: "/usr/bin",
      },
    })
    expect(result.env["AGENT_TERMINAL_SCOPE"]).toBe("abc-def-123")
  })

  it("honors explicit AGENT_TERMINAL_SCOPE over session id", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: {
        AGENT_TERMINAL_SCOPE: "explicit",
        PI_SESSION_ID: "session-id",
        PATH: "/usr/bin",
      },
    })
    expect(result.env["AGENT_TERMINAL_SCOPE"]).toBe("explicit")
  })

  it("does not set AGENT_TERMINAL_SCOPE when no session id is available", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: { PATH: "/usr/bin" },
    })
    expect(result.env["AGENT_TERMINAL_SCOPE"]).toBeUndefined()
  })

  it("prepends bundled agent-terminal bin dir to PATH", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: { PI_SESSION_ID: "s", PATH: "/usr/bin:/bin" },
    })
    expect(result.env["PATH"]).toContain("/fake/package/bin/linux-x64")
    // The original PATH entries should still be present
    expect(result.env["PATH"]).toContain("/usr/bin")
    expect(result.env["PATH"]).toContain("/bin")
  })

  it("prepends bundled zellij bin dir to PATH when not missing", () => {
    const hook = createSpawnHook("/fake/package", false)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: { PI_SESSION_ID: "s", PATH: "/usr/bin" },
    })
    expect(result.env["PATH"]).toContain("/fake/package/bin/zellij")
    expect(result.env["PATH"]).toContain("/fake/package/bin/linux-x64")
  })

  it("does not prepend zellij dir when missing", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: { PI_SESSION_ID: "s", PATH: "/usr/bin" },
    })
    expect(result.env["PATH"]).not.toContain("/fake/package/bin/zellij")
  })

  it("deduplicates PATH entries", () => {
    const hook = createSpawnHook("/fake/package", true)
    const result = hook({
      command: "echo hi",
      cwd: "/tmp",
      env: {
        PI_SESSION_ID: "s",
        PATH: "/fake/package/bin/linux-x64:/usr/bin",
      },
    })
    const pathParts = result.env["PATH"]?.split(":") ?? []
    const agentTerminalEntries = pathParts.filter((p) => p === "/fake/package/bin/linux-x64")
    expect(agentTerminalEntries.length).toBe(1)
  })
})
