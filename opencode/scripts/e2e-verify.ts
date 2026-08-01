import { existsSync, readFileSync } from "node:fs"

export type ToolState = {
  readonly status: string
  readonly input?: { readonly command?: string; readonly workdir?: string }
  readonly output?: string
  readonly error?: string
  readonly metadata?: Readonly<Record<string, unknown>>
}

export type ToolPart = {
  type: "tool"
  tool: string
  callID?: string
  state?: ToolState
}

export type TextPart = {
  type: "text"
  text?: string
}

export type StepEvent = {
  type: string
  part: ToolPart | TextPart | Record<string, unknown>
}

type Result = { ok: true } | { ok: false; error: string }

export type VerifyMode = "strict" | "real"

const AGENT_TERMINAL_SUBCOMMANDS = ["list", "start", "read", "send", "press", "stop"] as const
type AgentTerminalSubcommand = (typeof AGENT_TERMINAL_SUBCOMMANDS)[number]

type ParsedAgentCommand =
  | { readonly kind: "other" }
  | { readonly kind: "invalid"; readonly reason: string }
  | {
      readonly kind: "lifecycle"
      readonly command: string
      readonly subcommand: AgentTerminalSubcommand
    }

type ToolEvent = {
  readonly index: number
  readonly command: string
  readonly output: string
  readonly workdir?: string
}

export function parseTranscript(path: string): StepEvent[] {
  const text = readFileSyncUtf8(path)
  const lines = text.split(/\r?\n/).filter((line) => line.trim() !== "")
  const events: StepEvent[] = []
  for (const line of lines) {
    try {
      events.push(JSON.parse(line) as StepEvent)
    } catch {
      throw new Error(`transcript contains non-JSON line: ${line.slice(0, 200)}`)
    }
  }
  return events
}

function readFileSyncUtf8(path: string): string {
  if (!existsSync(path)) {
    throw new Error(`transcript file not found: ${path}`)
  }
  return readFileSync(path, "utf8")
}

function isToolPart(part: unknown): part is ToolPart {
  const p = part as Record<string, unknown> | undefined
  return p?.["type"] === "tool" && p?.["tool"] === "bash"
}

function isTextPart(part: unknown): part is TextPart {
  return (part as Record<string, unknown> | undefined)?.["type"] === "text"
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function parseJsonOutput(output: string | undefined): unknown {
  if (output === undefined || output === "") {
    return undefined
  }
  try {
    return JSON.parse(output)
  } catch {
    return undefined
  }
}

function hasShellSyntax(command: string): boolean {
  let quote: '"' | "'" | undefined
  let escaped = false
  for (const character of command) {
    if (escaped) {
      escaped = false
      continue
    }
    if (quote === undefined && character === "\\") {
      escaped = true
      continue
    }
    if (quote === undefined && (character === '"' || character === "'")) {
      quote = character
      continue
    }
    if (quote === '"' && character === "\\") {
      escaped = true
      continue
    }
    if (quote !== undefined && character === quote) {
      quote = undefined
      continue
    }
    if (quote !== "'" && (character === "`" || character === "$")) {
      return true
    }
    if (
      quote === undefined &&
      [";", "&", "|", "<", ">", "(", ")", "\n", "\r"].includes(character)
    ) {
      return true
    }
  }
  return quote !== undefined
}

function containsAgentTerminalToken(command: string): boolean {
  return /(?:^|[\s'"])agent-terminal\b/.test(command)
}

function parseAgentTerminalCommand(command: string | undefined): ParsedAgentCommand {
  if (!command) return { kind: "other" }
  const trimmed = command.trim()
  if (!/^agent-terminal(?:\s|$)/.test(trimmed)) {
    return containsAgentTerminalToken(trimmed)
      ? { kind: "invalid", reason: "agent-terminal is not the direct command" }
      : { kind: "other" }
  }
  if (containsAgentTerminalToken(trimmed.slice("agent-terminal".length))) {
    return { kind: "invalid", reason: "contains a nested agent-terminal invocation" }
  }
  if (hasShellSyntax(trimmed)) {
    return { kind: "invalid", reason: "contains shell syntax outside a quoted argument" }
  }
  const subcommand = /^agent-terminal\s+(\S+)/.exec(trimmed)?.[1]
  switch (subcommand) {
    case "list":
    case "start":
    case "read":
    case "send":
    case "press":
    case "stop":
      return { kind: "lifecycle", command: trimmed, subcommand }
    default:
      return { kind: "invalid", reason: `unsupported subcommand: ${subcommand ?? "missing"}` }
  }
}

function isSuccessfulJsonOutput(output: string): boolean {
  const parsed = parseJsonOutput(output)
  return isRecord(parsed) && parsed["status"] === "ok"
}

function hasNonZeroExit(state: ToolState): boolean {
  const rawExit =
    state.metadata?.["exit"] ?? state.metadata?.["exitCode"] ?? state.metadata?.["exit_code"]
  if (typeof rawExit === "number") return rawExit !== 0
  if (typeof rawExit !== "string" || rawExit.trim() === "") return false
  const parsedExit = Number(rawExit)
  return Number.isFinite(parsedExit) && parsedExit !== 0
}

function toolStateFailure(part: unknown): string | undefined {
  if (!isRecord(part) || !isRecord(part["state"])) return undefined
  const state = part["state"]
  const status = state["status"]
  if (status === "error" || status === "failed") {
    return `tool call failed: ${JSON.stringify(state).slice(0, 200)}`
  }
  if (state["error"] !== undefined && state["error"] !== null) {
    return `tool call failed: ${String(state["error"])}`
  }
  if (status === "completed") {
    const typedState = state as ToolState
    if (hasNonZeroExit(typedState)) {
      return `tool call returned a non-zero exit status: ${JSON.stringify(state).slice(0, 200)}`
    }
  }
  return undefined
}

function expectOkJson(
  output: string | undefined,
  check: (data: Record<string, unknown>) => Result,
): Result {
  const parsed = parseJsonOutput(output)
  if (parsed === undefined) {
    return { ok: false, error: `tool output is not valid JSON: ${output?.slice(0, 200)}` }
  }
  if (!isRecord(parsed)) {
    return { ok: false, error: `expected JSON object, got ${JSON.stringify(parsed)}` }
  }
  const obj = parsed
  if (obj["status"] !== "ok") {
    return { ok: false, error: `expected status ok, got ${JSON.stringify(obj)}` }
  }
  if (!isRecord(obj["data"])) {
    return { ok: false, error: `expected data object, got ${JSON.stringify(obj["data"])}` }
  }
  return check(obj["data"])
}

export function verifyE2E(
  path: string,
  jobName: string,
  mode: VerifyMode = "strict",
  expectedWorkdir?: string,
): Result {
  const events = parseTranscript(path)

  // Reject top-level error events (e.g., a tool execution failure surfaced as an error).
  for (const ev of events) {
    if (ev.type === "error") {
      return {
        ok: false,
        error: `transcript contains a top-level error event: ${JSON.stringify(ev.part).slice(0, 200)}`,
      }
    }
  }

  const toolEvents: ToolEvent[] = []
  for (let i = 0; i < events.length; i++) {
    const ev = events[i]
    if (ev === undefined) continue
    if (ev.type !== "tool_use") continue
    const part = ev.part
    const failure = toolStateFailure(part)
    if (failure !== undefined) return { ok: false, error: failure }
    if (!isToolPart(part)) {
      return {
        ok: false,
        error: `non-Bash tool call is outside the lifecycle contract: ${JSON.stringify(part).slice(0, 200)}`,
      }
    }
    // Reject any Bash tool call that is not a completed, successful invocation.
    if (part.state?.status !== "completed") {
      return {
        ok: false,
        error: `Bash tool call did not complete: ${JSON.stringify(part.state).slice(0, 200)}`,
      }
    }
    if (part.state?.error) {
      return { ok: false, error: `Bash tool call errored: ${part.state.error}` }
    }
    if (hasNonZeroExit(part.state)) {
      return {
        ok: false,
        error: `Bash tool call returned a non-zero exit status: ${JSON.stringify(part.state.metadata).slice(0, 200)}`,
      }
    }
    const workdir = part.state.input?.workdir
    if (expectedWorkdir !== undefined && workdir !== undefined && workdir !== expectedWorkdir) {
      return {
        ok: false,
        error: `Bash tool call used unexpected workdir: ${workdir}; expected ${expectedWorkdir}`,
      }
    }
    const parsedCommand = parseAgentTerminalCommand(part.state?.input?.command)
    if (parsedCommand.kind === "other") {
      return {
        ok: false,
        error: `Bash tool call is not an agent-terminal lifecycle command: ${part.state?.input?.command ?? ""}`,
      }
    }
    if (parsedCommand.kind === "invalid") {
      return {
        ok: false,
        error: `invalid agent-terminal command: ${parsedCommand.reason}: ${part.state?.input?.command ?? ""}`,
      }
    }
    toolEvents.push({
      index: i,
      command: parsedCommand.command,
      output: part.state.output ?? "",
      ...(workdir === undefined ? {} : { workdir }),
    })
  }

  if (toolEvents.length === 0) {
    return { ok: false, error: "no completed Bash agent-terminal tool_use events found" }
  }

  let step = 0
  const expectedCommands = {
    initialList: "agent-terminal list",
    start: `agent-terminal start ${jobName} -- /bin/bash -lc 'printf "prompt-ready\\n"; IFS= read -r first; printf "first:%s\\n" "$first"; IFS= read -r second; printf "second:%s\\n" "$second"'`,
    read: `agent-terminal read ${jobName}`,
    send: `agent-terminal send ${jobName} -- hello-e2e`,
    press: `agent-terminal press ${jobName} -- Enter`,
    stop: `agent-terminal stop ${jobName}`,
    finalList: "agent-terminal list",
  }
  const milestones = [
    {
      name: "initial list",
      command: expectedCommands.initialList,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const jobs = data["jobs"]
          if (!Array.isArray(jobs) || jobs.length !== 0) {
            return {
              ok: false,
              error: `initial list did not return empty jobs: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "start",
      command: expectedCommands.start,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data["job"] !== jobName || data["state"] !== "running") {
            return {
              ok: false,
              error: `start did not return running job ${jobName}: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "first read (prompt-ready)",
      command: expectedCommands.read,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const screen = String((data["screen"] as string) ?? (data["last_screen"] as string) ?? "")
          if (!screen.includes("prompt-ready")) {
            return {
              ok: false,
              error: `first read did not contain prompt-ready: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "send",
      command: expectedCommands.send,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data["job"] !== jobName || data["issued"] !== "text" || data["submitted"] !== true) {
            return {
              ok: false,
              error: `send did not return expected result: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "second read (first:hello-e2e)",
      command: expectedCommands.read,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const screen = String((data["screen"] as string) ?? (data["last_screen"] as string) ?? "")
          if (!screen.includes("first:hello-e2e")) {
            return {
              ok: false,
              error: `second read did not contain first:hello-e2e: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "press Enter",
      command: expectedCommands.press,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const keys = data["keys"]
          if (data["job"] !== jobName || !Array.isArray(keys) || !keys.includes("Enter")) {
            return { ok: false, error: `press did not return Enter keys: ${JSON.stringify(data)}` }
          }
          return { ok: true }
        }),
    },
    {
      name: "third read (exited)",
      command: expectedCommands.read,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data["job"] !== jobName || data["state"] !== "exited" || data["exit_code"] !== 0) {
            return {
              ok: false,
              error: `third read did not return exited/0: ${JSON.stringify(data)}`,
            }
          }
          const screen = String((data["screen"] as string) ?? (data["last_screen"] as string) ?? "")
          if (!screen.includes("second:")) {
            return {
              ok: false,
              error: `third read did not contain second:: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "stop",
      command: expectedCommands.stop,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data["job"] !== jobName || data["cleaned_up"] !== true) {
            return {
              ok: false,
              error: `stop did not return cleaned_up true: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "final list",
      command: expectedCommands.finalList,
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const jobs = data["jobs"]
          if (!Array.isArray(jobs) || jobs.length !== 0) {
            return {
              ok: false,
              error: `final list did not return empty jobs: ${JSON.stringify(data)}`,
            }
          }
          return { ok: true }
        }),
    },
  ]

  const attempts: number[] = new Array(milestones.length).fill(0)
  const maxAttempts = mode === "real" ? 8 : 3

  for (const ev of toolEvents) {
    if (step >= milestones.length) {
      return {
        ok: false,
        error: `lifecycle complete but an extra agent-terminal Bash call followed: ${ev.command}`,
      }
    }
    const milestone = milestones[step]
    if (milestone === undefined) {
      return { ok: false, error: `invalid lifecycle state at milestone ${step + 1}` }
    }
    if (ev.command !== milestone.command) {
      return {
        ok: false,
        error: `out-of-order or extraneous agent-terminal Bash call at milestone ${step + 1}: ${ev.command}`,
      }
    }
    attempts[step] = (attempts[step] ?? 0) + 1
    const attemptCount = attempts[step] ?? 0
    const result = milestone.check(ev.output)
    if (!result.ok) {
      if (!isSuccessfulJsonOutput(ev.output)) {
        return {
          ok: false,
          error: `milestone "${milestone.name}" failed: ${result.error}`,
        }
      }
      if (attemptCount >= maxAttempts) {
        return {
          ok: false,
          error: `milestone "${milestone.name}" failed after ${maxAttempts} attempts: ${result.error}`,
        }
      }
      continue
    }
    step++
  }

  if (step < milestones.length) {
    const pending = milestones
      .slice(step)
      .map((m) => m.name)
      .join(", ")
    return { ok: false, error: `lifecycle incomplete. missing milestones: ${pending}` }
  }

  const finalToolIndex = toolEvents[toolEvents.length - 1]?.index
  if (finalToolIndex === undefined) {
    return { ok: false, error: "lifecycle completed without a final tool event" }
  }
  let lastTextIndex = -1
  let lastTextHasSuccess = false
  for (let i = 0; i < events.length; i++) {
    const ev = events[i]
    if (ev === undefined || ev.type !== "text" || !isTextPart(ev.part) || !ev.part.text) continue
    lastTextIndex = i
    lastTextHasSuccess = ev.part.text.split(/\r?\n/).some((line) => line.trim() === "E2E_SUCCESS")
  }
  if (lastTextIndex <= finalToolIndex || !lastTextHasSuccess) {
    return { ok: false, error: "no standalone E2E_SUCCESS line found in final assistant text" }
  }

  // Strict mode rejects Markdown code blocks that re-print agent-terminal commands,
  // because the fixture model must drive the lifecycle exclusively through tool
  // calls. Real models commonly summarize their actions in a final code block
  // AFTER executing them, so this check does not apply in real mode.
  if (mode === "strict") {
    for (const ev of events) {
      if (ev.type === "text" && isTextPart(ev.part) && ev.part.text) {
        const text = ev.part.text
        const fenceMatch = text.match(/```[a-z]*\n([\s\S]*?)```/g)
        if (fenceMatch) {
          for (const block of fenceMatch) {
            if (/^\s*agent-terminal\b/m.test(block)) {
              return {
                ok: false,
                error:
                  "model emitted agent-terminal commands inside a Markdown code block instead of using tool calls",
              }
            }
          }
        }
      }
    }
  }

  return { ok: true }
}

function main(): void {
  const args = process.argv.slice(2)
  let path = ""
  let jobName = ""
  let mode: VerifyMode = "strict"
  let expectedWorkdir: string | undefined
  for (let i = 0; i < args.length; i++) {
    const argument = args[i]
    if (argument === undefined) continue
    if (argument === "--mode" && i + 1 < args.length) {
      const v = args[i + 1]
      mode = v === "real" ? "real" : "strict"
      i++
      continue
    }
    if (argument === "--workdir" && i + 1 < args.length) {
      expectedWorkdir = args[i + 1]
      i++
      continue
    }
    if (!path) path = argument
    else if (!jobName) jobName = argument
  }
  if (!path || !jobName) {
    console.error(
      "usage: bun run e2e-verify.ts <transcript.jsonl> <job-name> [--mode strict|real] [--workdir path]",
    )
    process.exit(2)
  }
  const result = verifyE2E(path, jobName, mode, expectedWorkdir)
  if (!result.ok) {
    console.error(`e2e verification failed: ${result.error}`)
    process.exit(1)
  }
  console.log("e2e verification passed")
}

if (import.meta.main) {
  main()
}
