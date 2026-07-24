import { afterEach, describe, expect, test } from "bun:test"
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import type { ToolContext } from "@opencode-ai/plugin"

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
    const tools = await import("../tools/terminal")
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
      const { start } = await import("../tools/terminal")
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
      const { read } = await import("../tools/terminal")
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
      const { list } = await import("../tools/terminal")

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
      const { press } = await import("../tools/terminal")
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

  test("send defaults to submit and explicit flags map to controller argv", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      const capture = join(directory, "argv")
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: {},
      })
      process.env["AGENT_TERMINAL_CAPTURE"] = capture
      const { send, stop } = await import("../tools/terminal")
      const context = toolContext(directory)

      await Reflect.apply(send.execute, send, [{ job: "repl", text: "hello" }, context])
      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toEqual([
        "--project",
        directory,
        "send",
        "repl",
        "--",
        "hello",
      ])

      await send.execute({ job: "repl", text: "hello", submit: false }, context)
      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toContain("--no-submit")

      await stop.execute({ job: "repl", force: true }, context)
      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toContain("--force")
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  test("all tools request named permissions and start also requests bash", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: {},
      })
      const tools = await import("../tools/terminal")
      const permissions: string[] = []
      const context = toolContext(
        directory,
        new AbortController().signal,
        directory,
        async (input) => {
          permissions.push(input.permission)
        },
      )

      await tools.start.execute({ job: "server", command: "npm run dev", cwd: directory }, context)
      await tools.read.execute({ job: "server" }, context)
      await tools.send.execute({ job: "server", text: "status", submit: true }, context)
      await tools.press.execute({ job: "server", keys: ["Enter"] }, context)
      await tools.stop.execute({ job: "server", force: false }, context)
      await tools.list.execute({}, context)

      expect(permissions).toEqual([
        "terminal_start",
        "bash",
        "terminal_read",
        "terminal_send",
        "terminal_press",
        "terminal_stop",
        "terminal_list",
      ])
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  test("permission rejection prevents spawning and external cwd asks separately", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    const outside = await mkdtemp(join(tmpdir(), "agent-terminal-outside-"))
    try {
      const capture = join(directory, "argv")
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: {},
      })
      process.env["AGENT_TERMINAL_CAPTURE"] = capture
      const { list, start } = await import("../tools/terminal")
      const denied = toolContext(directory, new AbortController().signal, directory, async () => {
        throw new Error("permission denied")
      })

      await expect(list.execute({}, denied)).rejects.toThrow("permission denied")
      await expect(readFile(capture, "utf8")).rejects.toThrow()

      const permissions: string[] = []
      const allowed = toolContext(
        directory,
        new AbortController().signal,
        directory,
        async (input) => {
          permissions.push(input.permission)
        },
      )
      await start.execute({ job: "server", command: "pwd", cwd: outside }, allowed)
      expect(permissions).toEqual(["terminal_start", "external_directory", "bash"])
    } finally {
      await rm(directory, { recursive: true, force: true })
      await rm(outside, { recursive: true, force: true })
    }
  })

  test("cwd authorization and execution use the same canonical path", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    const outside = await mkdtemp(join(tmpdir(), "agent-terminal-outside-"))
    try {
      const inside = join(directory, "inside")
      const escapePath = join(directory, "escape")
      const capture = join(directory, "argv")
      await mkdir(inside)
      await symlink(outside, escapePath)
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: {},
      })
      process.env["AGENT_TERMINAL_CAPTURE"] = capture
      const { start } = await import("../tools/terminal")
      const requests: Parameters<ToolContext["ask"]>[0][] = []
      const context = toolContext(
        directory,
        new AbortController().signal,
        directory,
        async (input) => {
          requests.push(input)
        },
      )

      await start.execute({ job: "inside", command: "pwd", cwd: "inside" }, context)
      expect(requests.map((request) => request.permission)).toEqual(["terminal_start", "bash"])
      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toContain(
        await realpath(inside),
      )

      requests.length = 0
      await start.execute({ job: "escape", command: "pwd", cwd: "escape" }, context)
      expect(requests.map((request) => request.permission)).toEqual([
        "terminal_start",
        "external_directory",
        "bash",
      ])
      expect(requests[1]?.patterns).toEqual([await realpath(outside)])
      expect((await readFile(capture, "utf8")).split("\n").filter(Boolean)).toContain(
        await realpath(outside),
      )
    } finally {
      await rm(directory, { recursive: true, force: true })
      await rm(outside, { recursive: true, force: true })
    }
  })

  test("an already-aborted call rejects without leaving a child", async () => {
    const directory = await mkdtemp(join(tmpdir(), "agent-terminal-adapter-"))
    try {
      process.env["AGENT_TERMINAL_BIN"] = await stubController(directory, 0, {
        status: "ok",
        data: { jobs: [] },
      })
      const { list } = await import("../tools/terminal")
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
  ask: ToolContext["ask"] = async () => {},
): ToolContext {
  return {
    sessionID: "session",
    messageID: "message",
    agent: "build",
    directory,
    worktree,
    abort,
    metadata() {},
    ask,
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
