import { afterEach, describe, expect, test } from "bun:test"
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

const originalBinary = process.env["AGENT_TERMINAL_BIN"]
const originalCapture = process.env["AGENT_TERMINAL_CAPTURE"]
const originalShell = process.env["SHELL"]

afterEach(() => {
  restoreEnv("AGENT_TERMINAL_BIN", originalBinary)
  restoreEnv("AGENT_TERMINAL_CAPTURE", originalCapture)
  restoreEnv("SHELL", originalShell)
})

describe("OpenCode terminal tools", () => {
  test("exports exactly six narrow tools", async () => {
    const tools = await import("../terminal")
    expect(Object.keys(tools).sort()).toEqual(["list", "press", "read", "send", "start", "stop"])
  })

  test("start maps context and shell command to structured controller argv", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      const capture = join(directory, "argv")
      const binary = await stubController(directory, 0, {
        status: "ok",
        data: { job: "server", state: "running" },
      })
      process.env["AGENT_TERMINAL_BIN"] = binary
      process.env["AGENT_TERMINAL_CAPTURE"] = capture
      process.env["SHELL"] = "/bin/sh"
      const { start } = await import("../terminal")
      const context: Parameters<typeof start.execute>[1] = toolContext(directory)

      const output = await start.execute(
        { job: "server", command: "npm run dev", cwd: directory },
        context,
      )
      if (typeof output !== "string") throw new Error("expected textual tool output")

      expect(JSON.parse(output)).toEqual({
        status: "ok",
        data: { job: "server", state: "running" },
      })
      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toEqual([
        "--project",
        directory,
        "start",
        "server",
        "--cwd",
        directory,
        "--",
        "/bin/sh",
        "-c",
        "npm run dev",
      ])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  test("valid controller error envelopes are returned instead of thrown", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 1, {
        status: "error",
        error: { code: "job_not_found", message: "missing" },
      })
      const { read } = await import("../terminal")
      const output = await read.execute({ job: "missing" }, toolContext(directory))
      if (typeof output !== "string") throw new Error("expected textual tool output")
      expect(JSON.parse(output)).toEqual({
        status: "error",
        error: { code: "job_not_found", message: "missing" },
      })
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  test("uses directory scope when OpenCode reports the filesystem root as worktree", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      const capture = join(directory, "argv")
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: { jobs: [] },
      })
      process.env["AGENT_TERMINAL_CAPTURE"] = capture
      const { list } = await import("../terminal")

      await list.execute({}, toolContext(directory, new AbortController().signal, "/"))

      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toEqual([
        "--project",
        directory,
        "list",
      ])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  test("press validates public keys and separates argv", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      const capture = join(directory, "argv")
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: { job: "debugger", issued: "keys", keys: ["Alt+!"] },
      })
      process.env["AGENT_TERMINAL_CAPTURE"] = capture
      const { press } = await import("../terminal")
      expect(press.args.keys.safeParse(["Alt+!"]).success).toBe(true)
      expect(press.args.keys.safeParse(["Ctrl+7"]).success).toBe(false)

      await press.execute({ job: "debugger", keys: ["Alt+!"] }, toolContext(directory))

      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toEqual([
        "--project",
        directory,
        "press",
        "debugger",
        "--",
        "Alt+!",
      ])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  test("an already-aborted call rejects without leaving a child", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: { jobs: [] },
      })
      const { list } = await import("../terminal")
      const abort = new AbortController()
      abort.abort()
      await expect(list.execute({}, toolContext(directory, abort.signal))).rejects.toThrow()
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })
})

function toolContext(
  directory: string,
  abort: AbortSignal = new AbortController().signal,
  worktree: string = directory,
) {
  return {
    sessionID: "session",
    messageID: "message",
    callID: "call",
    agent: "build",
    directory,
    worktree,
    abort,
    metadata() {},
    async ask() {},
  }
}

async function stubController(
  directory: string,
  exitCode: number,
  response: object,
): Promise<string> {
  const path = join(directory, "controller")
  await writeFile(
    path,
    `#!/bin/sh\nif [ -n "$AGENT_TERMINAL_CAPTURE" ]; then printf '%s\\n' "$@" > "$AGENT_TERMINAL_CAPTURE"; fi\nprintf '%s\\n' '${JSON.stringify(response)}'\nexit ${exitCode}\n`,
  )
  await chmod(path, 0o700)
  return path
}

function restoreEnv(name: string, value: string | undefined): void {
  if (value === undefined) {
    delete process.env[name]
  } else {
    process.env[name] = value
  }
}
