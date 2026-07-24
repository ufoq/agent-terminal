import { spawn } from "node:child_process"
import { isAbsolute, parse, resolve } from "node:path"
import { tool, type ToolContext, type ToolResult } from "@opencode-ai/plugin"

const job = tool.schema
  .string()
  .regex(/^[a-z0-9][a-z0-9._-]{0,63}$/)
  .describe("Project-scoped job name")
const key = tool.schema
  .string()
  .regex(
    /^(Enter|Tab|Esc|Backspace|Delete|Insert|Home|End|PageUp|PageDown|Up|Down|Left|Right|F(?:[1-9]|1[0-2])|Ctrl\+[A-Za-z]|Alt\+[ -~])$/,
  )
const envelope = tool.schema.discriminatedUnion("status", [
  tool.schema.object({ status: tool.schema.literal("ok"), data: tool.schema.unknown() }),
  tool.schema.object({
    status: tool.schema.literal("error"),
    error: tool.schema.object({
      code: tool.schema.string(),
      message: tool.schema.string(),
      hint: tool.schema.string().optional(),
    }),
  }),
])

export const start = tool({
  description:
    "Start a persistent or interactive terminal job. Use normal shell tools for short foreground commands.",
  args: {
    job,
    command: tool.schema.string().min(1).describe("Shell command to run"),
    cwd: tool.schema
      .string()
      .min(1)
      .optional()
      .describe("Working directory; defaults to current directory"),
  },
  async execute(args, context) {
    const cwd = args.cwd ?? context.directory
    return runController(
      [
        "--project",
        projectScope(context),
        "start",
        args.job,
        "--cwd",
        cwd,
        "--",
        commandShell(),
        "-c",
        args.command,
      ],
      context,
    )
  },
})

export const read = tool({
  description: "Read a terminal job's current lifecycle state and bounded visible screen.",
  args: { job },
  async execute(args, context) {
    return runController(["--project", projectScope(context), "read", args.job], context)
  },
})

export const send = tool({
  description: "Send literal text to a running terminal job, pressing Enter by default.",
  args: {
    job,
    text: tool.schema.string().min(1),
    submit: tool.schema.boolean().default(true),
  },
  async execute(args, context) {
    const controllerArgs = ["--project", projectScope(context), "send", args.job]
    if (!args.submit) controllerArgs.push("--no-submit")
    controllerArgs.push("--", args.text)
    return runController(controllerArgs, context)
  },
})

export const press = tool({
  description: "Press canonical named keys in a running terminal job. Use stop to terminate a job.",
  args: { job, keys: tool.schema.array(key).min(1) },
  async execute(args, context) {
    return runController(
      ["--project", projectScope(context), "press", args.job, "--", ...args.keys],
      context,
    )
  },
})

export const stop = tool({
  description:
    "Clean up a terminal job gracefully; set force only after graceful stop reports it is still running.",
  args: { job, force: tool.schema.boolean().default(false) },
  async execute(args, context) {
    const controllerArgs = ["--project", projectScope(context), "stop", args.job]
    if (args.force) controllerArgs.push("--force")
    return runController(controllerArgs, context)
  },
})

export const list = tool({
  description:
    "List project-scoped terminal jobs and their lifecycle state; use after context loss.",
  args: {},
  async execute(_args, context) {
    return runController(["--project", projectScope(context), "list"], context)
  },
})

async function runController(
  arguments_: readonly string[],
  context: ToolContext,
): Promise<ToolResult> {
  const result = await spawnController(arguments_, context.abort)
  const stdout = result.stdout.trim()
  if (stdout.length === 0) {
    throw new ControllerExecutionError(result.code, result.signal, result.stderr)
  }
  let decoded: unknown
  try {
    decoded = JSON.parse(stdout)
  } catch (error) {
    throw new ControllerProtocolError("controller stdout was not JSON", { cause: error })
  }
  const parsed = envelope.safeParse(decoded)
  if (!parsed.success) {
    throw new ControllerProtocolError(
      `controller response failed schema validation: ${parsed.error.message}`,
    )
  }
  return JSON.stringify(parsed.data)
}

function spawnController(
  arguments_: readonly string[],
  abort: AbortSignal,
): Promise<{
  readonly code: number | null
  readonly signal: NodeJS.Signals | null
  readonly stdout: string
  readonly stderr: string
}> {
  return new Promise((resolve, reject) => {
    const child = spawn(controllerBinary(), arguments_, {
      signal: abort,
      stdio: ["ignore", "pipe", "pipe"],
    })
    const stdout: Uint8Array[] = []
    const stderr: Uint8Array[] = []
    child.stdout.on("data", (chunk: Uint8Array) => stdout.push(chunk))
    child.stderr.on("data", (chunk: Uint8Array) => stderr.push(chunk))
    child.once("error", reject)
    child.once("close", (code, signal) => {
      resolve({
        code,
        signal,
        stdout: Buffer.concat(stdout).toString("utf8"),
        stderr: Buffer.concat(stderr).toString("utf8"),
      })
    })
  })
}

function controllerBinary(): string {
  const configured = process.env["AGENT_TERMINAL_BIN"]
  return configured && configured.length > 0 ? configured : "agent-terminal"
}

function commandShell(): string {
  const configured = process.env["SHELL"]
  return configured && isAbsolute(configured) ? configured : "/bin/sh"
}

function projectScope(context: ToolContext): string {
  const worktree = resolve(context.worktree)
  const directory = resolve(context.directory)
  return worktree === parse(worktree).root && directory !== worktree ? directory : worktree
}

class ControllerExecutionError extends Error {
  readonly name = "ControllerExecutionError"

  constructor(code: number | null, signal: NodeJS.Signals | null, stderr: string) {
    super(
      `controller produced no JSON (exit=${String(code)}, signal=${String(signal)}): ${stderr.trim()}`,
    )
  }
}

class ControllerProtocolError extends Error {
  readonly name = "ControllerProtocolError"
}
