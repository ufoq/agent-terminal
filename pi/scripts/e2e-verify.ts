import { existsSync, readFileSync } from "node:fs"

export type PiEvent = {
  readonly type: string
  readonly id?: string
  readonly toolCallId?: string
  readonly toolName?: string
  readonly args?: Readonly<Record<string, unknown>>
  readonly result?: Readonly<Record<string, unknown>>
  readonly isError?: boolean
  readonly message?: Readonly<Record<string, unknown>>
  readonly errorMessage?: unknown
}

export type ToolExecution = {
  readonly index: number
  readonly command: string
  readonly output: string
  readonly workdir?: string
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

export function parseTranscript(path: string): PiEvent[] {
  const text = readFileSyncUtf8(path)
  const lines = text.split(/\r?\n/).filter((line) => line.trim() !== "")
  const events: PiEvent[] = []
  for (const line of lines) {
    try {
      events.push(JSON.parse(line) as PiEvent)
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function parseJsonOutput(output: string | undefined): unknown {
  if (output === undefined || output === "") {
    return undefined
  }
  const trimmed = output.trim()
  try {
    return JSON.parse(trimmed)
  } catch {
    // omp's bash tool appends "\n\nWall time: X seconds\n\nCommand exited with code N"
    // after the actual output. Try to extract the first JSON object.
    const jsonStart = trimmed.indexOf("{")
    if (jsonStart < 0) return undefined
    for (let end = trimmed.length; end > jsonStart; end--) {
      const candidate = trimmed.slice(jsonStart, end)
      try {
        return JSON.parse(candidate)
      } catch {}
    }
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

// The scope probe is the first Bash call in the deterministic gate. It proves
// the extension's spawnHook set AGENT_TERMINAL_SCOPE to the pi session id
// (which the model does not supply), by printing it back into the tool
// result. The factory-time process.env.PATH mutation alone could expose the
// binary while scope silently fell back to "standalone", so this probe (and
// its exact-match check) is what actually guards cross-agent isolation.
const SCOPE_PROBE_COMMAND = "printenv AGENT_TERMINAL_SCOPE"

function isScopeProbeCommand(command: string | undefined): boolean {
  return command?.trim() === SCOPE_PROBE_COMMAND
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

function expectOkJson(
  output: string | undefined,
  check: (body: Record<string, unknown>) => Result,
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
  return check(obj)
}

// pi's bash tool result is `{ content: [{ type: "text", text: "<output>" }], details }`;
// the command output is the first text content part.
function extractToolOutput(result: Readonly<Record<string, unknown>> | undefined): string {
  if (result === undefined) return ""
  const content = result["content"]
  if (!Array.isArray(content)) return ""
  for (const part of content) {
    if (!isRecord(part)) continue
    if (part["type"] !== "text") continue
    const text = part["text"]
    if (typeof text === "string") return text
  }
  return ""
}

// pi sends assistant message content as arrays of `{ type: "text", text }`
// parts, but a plain string is accepted too.
function extractMessageText(message: Readonly<Record<string, unknown>>): string | undefined {
  const content = message["content"]
  if (typeof content === "string") return content
  if (!Array.isArray(content)) return undefined
  const parts: string[] = []
  for (const part of content) {
    if (!isRecord(part)) continue
    if (part["type"] !== "text") continue
    const text = part["text"]
    if (typeof text === "string") parts.push(text)
  }
  return parts.length === 0 ? undefined : parts.join("\n")
}

export function verifyE2E(
  path: string,
  jobName: string,
  mode: VerifyMode = "strict",
  expectedWorkdir?: string,
): Result {
  const events = parseTranscript(path)

  // Strict mode rejects error events (auto_retry_start, or any event carrying
  // an errorMessage) because the deterministic gate must complete without
  // retries or failures. Real models may legitimately retry, so this check
  // does not apply in real mode.
  if (mode === "strict") {
    for (const ev of events) {
      if (ev.type === "auto_retry_start") {
        return { ok: false, error: "transcript contains an auto_retry_start event" }
      }
      if (ev.errorMessage !== undefined && ev.errorMessage !== null) {
        return {
          ok: false,
          error: `transcript contains an event with an errorMessage: ${JSON.stringify(ev.errorMessage).slice(0, 200)}`,
        }
      }
    }
  }

  // The first line of pi's `--mode json` stream is the session header, whose
  // `id` is the value the extension's spawnHook must have injected as
  // AGENT_TERMINAL_SCOPE.
  const header = events[0]
  if (header === undefined || header.type !== "session") {
    return { ok: false, error: "transcript does not start with a session header line" }
  }
  const sessionId = header.id
  if (sessionId === undefined || sessionId.trim() === "") {
    return { ok: false, error: "transcript session header does not carry an id" }
  }

  const toolEvents: ToolExecution[] = []
  let scopeProbe: ToolExecution | undefined
  const pendingBash = new Map<
    string,
    { readonly index: number; readonly command: string; readonly workdir?: string }
  >()

  for (let i = 0; i < events.length; i++) {
    const ev = events[i]
    if (ev === undefined) continue
    if (ev.type === "tool_execution_start") {
      if (ev.toolName !== "bash") {
        if (mode === "strict") {
          return {
            ok: false,
            error: `non-Bash tool call is outside the lifecycle contract: ${JSON.stringify(ev).slice(0, 200)}`,
          }
        }
        continue
      }
      const callId = ev.toolCallId
      if (callId === undefined) {
        return {
          ok: false,
          error: `Bash tool_execution_start without a toolCallId: ${JSON.stringify(ev).slice(0, 200)}`,
        }
      }
      const command = ev.args?.["command"]
      if (typeof command !== "string") {
        return {
          ok: false,
          error: `Bash tool_execution_start without a command argument: ${JSON.stringify(ev).slice(0, 200)}`,
        }
      }
      const workdir = ev.args?.["workdir"]
      pendingBash.set(callId, {
        index: i,
        command,
        ...(typeof workdir === "string" ? { workdir } : {}),
      })
      continue
    }
    if (ev.type === "tool_execution_end") {
      const callId = ev.toolCallId
      if (callId === undefined) continue
      const start = pendingBash.get(callId)
      if (start === undefined) continue
      pendingBash.delete(callId)
      if (ev.isError === true) {
        return {
          ok: false,
          error: `Bash tool call failed: ${JSON.stringify(ev.result).slice(0, 200)}`,
        }
      }
      const execution: ToolExecution = {
        index: i,
        command: start.command,
        output: extractToolOutput(ev.result),
        ...(start.workdir === undefined ? {} : { workdir: start.workdir }),
      }
      if (isScopeProbeCommand(execution.command)) {
        if (scopeProbe !== undefined) {
          return { ok: false, error: "the scope probe Bash call appeared more than once" }
        }
        if (toolEvents.length !== 0) {
          return {
            ok: false,
            error: `the scope probe must be the first Bash call, but a lifecycle call preceded it: ${toolEvents[0]?.command ?? ""}`,
          }
        }
        scopeProbe = execution
        continue
      }
      const workdir = execution.workdir
      if (expectedWorkdir !== undefined && workdir !== undefined && workdir !== expectedWorkdir) {
        return {
          ok: false,
          error: `Bash tool call used unexpected workdir: ${workdir}; expected ${expectedWorkdir}`,
        }
      }
      const parsedCommand = parseAgentTerminalCommand(execution.command)
      if (parsedCommand.kind === "other") {
        return {
          ok: false,
          error: `Bash tool call is not an agent-terminal lifecycle command: ${execution.command}`,
        }
      }
      if (parsedCommand.kind === "invalid") {
        return {
          ok: false,
          error: `invalid agent-terminal command: ${parsedCommand.reason}: ${execution.command}`,
        }
      }
      toolEvents.push({
        index: execution.index,
        command: parsedCommand.command,
        output: execution.output,
        ...(workdir === undefined ? {} : { workdir }),
      })
    }
  }

  if (pendingBash.size > 0) {
    return { ok: false, error: "a Bash tool call did not complete (missing tool_execution_end)" }
  }

  if (toolEvents.length === 0) {
    return { ok: false, error: "no completed Bash agent-terminal tool calls found" }
  }

  // The scope probe must be present, and its printed value must exactly match
  // the transcript's pi session id in BOTH modes. The session id and the
  // extension's env propagation are pi-owned, not model-dependent, so a
  // missing/empty/"standalone"/mismatched scope fails the real-model path
  // just as it fails the deterministic gate.
  if (scopeProbe === undefined) {
    return {
      ok: false,
      error: "the scope probe Bash call (printenv AGENT_TERMINAL_SCOPE) is missing",
    }
  }
  // omp's bash tool appends "\n\nWall time: X seconds" after the actual output.
  const printed = scopeProbe.output.replace(/\n\nWall time:.*$/s, "").trim()
  if (printed !== sessionId) {
    return {
      ok: false,
      error: `AGENT_TERMINAL_SCOPE (${printed}) does not match the pi session id (${sessionId})`,
    }
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
        expectOkJson(output, (body) => {
          const jobs = body["jobs"]
          if (!Array.isArray(jobs) || jobs.length !== 0) {
            return {
              ok: false,
              error: `initial list did not return empty jobs: ${JSON.stringify(body)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "start",
      command: expectedCommands.start,
      check: (output: string) =>
        expectOkJson(output, (body) => {
          if (body["state"] !== "running") {
            return {
              ok: false,
              error: `start did not return running state: ${JSON.stringify(body)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "first read (prompt-ready)",
      command: expectedCommands.read,
      check: (output: string) =>
        expectOkJson(output, (body) => {
          const screen = body["screen"]
          if (typeof screen !== "string" || !screen.includes("prompt-ready")) {
            return {
              ok: false,
              error: `first read did not contain prompt-ready: ${JSON.stringify(body)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "send",
      command: expectedCommands.send,
      check: (output: string) => expectOkJson(output, () => ({ ok: true })),
    },
    {
      name: "second read (first:hello-e2e)",
      command: expectedCommands.read,
      check: (output: string) =>
        expectOkJson(output, (body) => {
          const screen = body["screen"]
          if (typeof screen !== "string" || !screen.includes("first:hello-e2e")) {
            return {
              ok: false,
              error: `second read did not contain first:hello-e2e: ${JSON.stringify(body)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "press Enter",
      command: expectedCommands.press,
      check: (output: string) => expectOkJson(output, () => ({ ok: true })),
    },
    {
      name: "third read (exited)",
      command: expectedCommands.read,
      check: (output: string) =>
        expectOkJson(output, (body) => {
          if (body["state"] !== "exited" || body["exit_code"] !== 0) {
            return {
              ok: false,
              error: `third read did not return exited/0: ${JSON.stringify(body)}`,
            }
          }
          const screen = body["screen"]
          if (typeof screen !== "string" || !screen.includes("second:")) {
            return {
              ok: false,
              error: `third read did not contain second:: ${JSON.stringify(body)}`,
            }
          }
          return { ok: true }
        }),
    },
    {
      name: "stop",
      command: expectedCommands.stop,
      check: (output: string) => expectOkJson(output, () => ({ ok: true })),
    },
    {
      name: "final list",
      command: expectedCommands.finalList,
      check: (output: string) =>
        expectOkJson(output, (body) => {
          const jobs = body["jobs"]
          if (!Array.isArray(jobs) || jobs.length !== 0) {
            return {
              ok: false,
              error: `final list did not return empty jobs: ${JSON.stringify(body)}`,
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
    if (ev === undefined || ev.type !== "message_end") continue
    const message = ev.message
    if (!isRecord(message) || message["role"] !== "assistant") continue
    const text = extractMessageText(message)
    if (text === undefined || text === "") continue
    lastTextIndex = i
    lastTextHasSuccess = text.split(/\r?\n/).some((line) => line.trim() === "E2E_SUCCESS")
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
      if (ev.type !== "message_end") continue
      const message = ev.message
      if (!isRecord(message) || message["role"] !== "assistant") continue
      const text = extractMessageText(message)
      if (text === undefined) continue
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

  return { ok: true }
}

export function main(): void {
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
