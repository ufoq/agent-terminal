import { existsSync, readFileSync } from "node:fs"

export type ToolState = {
  status: string
  input?: { command?: string }
  output?: string
  error?: string
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
  return p?.type === "tool" && p?.tool === "bash"
}

function isTextPart(part: unknown): part is TextPart {
  return (part as Record<string, unknown> | undefined)?.type === "text"
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

function extractAgentTerminalCommand(command: string | undefined): string | null {
  if (!command) return null
  const trimmed = command.trim()
  if (!/^agent-terminal\b/.test(trimmed)) return null
  return trimmed
}

function expectOkJson(
  output: string | undefined,
  check: (data: Record<string, unknown>) => Result,
): Result {
  const parsed = parseJsonOutput(output)
  if (parsed === undefined) {
    return { ok: false, error: `tool output is not valid JSON: ${output?.slice(0, 200)}` }
  }
  const obj = parsed as Record<string, unknown>
  if (obj.status !== "ok") {
    return { ok: false, error: `expected status ok, got ${JSON.stringify(obj)}` }
  }
  if (typeof obj.data !== "object" || obj.data === null) {
    return { ok: false, error: `expected data object, got ${JSON.stringify(obj.data)}` }
  }
  return check(obj.data as Record<string, unknown>)
}

export function verifyE2E(path: string, jobName: string, mode: VerifyMode = "strict"): Result {
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

  const toolEvents: { index: number; callID: string; command: string; output: string }[] = []
  for (let i = 0; i < events.length; i++) {
    const ev = events[i]
    if (ev.type !== "tool_use") continue
    const part = ev.part
    if (!isToolPart(part)) {
      // In strict mode any non-Bash tool call fails the Bash-only contract.
      // In real mode a model may legitimately use other tools; those are
      // non-evidence and do not contribute to the lifecycle milestones.
      if (mode === "strict") {
        return {
          ok: false,
          error: `non-Bash tool call used in strict mode: ${JSON.stringify(part).slice(0, 200)}`,
        }
      }
      continue
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
    const callID = part.callID
    if (!callID) continue
    const command = extractAgentTerminalCommand(part.state?.input?.command)
    if (!command) {
      // Same policy as non-Bash tools: strict rejects, real treats as non-evidence.
      if (mode === "strict") {
        return {
          ok: false,
          error: `Bash tool call is not an agent-terminal command in strict mode: ${part.state?.input?.command ?? ""}`,
        }
      }
      continue
    }
    toolEvents.push({ index: i, callID, command, output: part.state.output ?? "" })
  }

  if (toolEvents.length === 0) {
    return { ok: false, error: "no completed Bash agent-terminal tool_use events found" }
  }

  let step = 0
  const milestones = [
    {
      name: "initial list",
      match: (cmd: string) =>
        /\blist\b/.test(cmd) && !/\bstart\b|\bread\b|\bsend\b|\bpress\b|\bstop\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const jobs = data.jobs
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
      match: (cmd: string) => /\bstart\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data.job !== jobName || data.state !== "running") {
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
      match: (cmd: string) => /\bread\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const screen = String((data.screen as string) ?? (data.last_screen as string) ?? "")
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
      match: (cmd: string) => /\bsend\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data.job !== jobName || data.issued !== "text" || data.submitted !== true) {
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
      match: (cmd: string) => /\bread\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const screen = String((data.screen as string) ?? (data.last_screen as string) ?? "")
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
      match: (cmd: string) => /\bpress\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const keys = data.keys
          if (data.job !== jobName || !Array.isArray(keys) || !keys.includes("Enter")) {
            return { ok: false, error: `press did not return Enter keys: ${JSON.stringify(data)}` }
          }
          return { ok: true }
        }),
    },
    {
      name: "third read (exited)",
      match: (cmd: string) => /\bread\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data.job !== jobName || data.state !== "exited" || data.exit_code !== 0) {
            return {
              ok: false,
              error: `third read did not return exited/0: ${JSON.stringify(data)}`,
            }
          }
          const screen = String((data.screen as string) ?? (data.last_screen as string) ?? "")
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
      match: (cmd: string) => /\bstop\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          if (data.job !== jobName || data.cleaned_up !== true) {
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
      match: (cmd: string) => /\blist\b/.test(cmd),
      check: (output: string) =>
        expectOkJson(output, (data) => {
          const jobs = data.jobs
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
  const isObservational = (cmd: string) => /\blist\b|\bread\b/.test(cmd)

  for (const ev of toolEvents) {
    if (step >= milestones.length) {
      if (mode === "real" && isObservational(ev.command)) {
        // A real model may re-check state after the lifecycle completed; that is
        // non-evidence and does not invalidate the completed milestones.
        continue
      }
      return {
        ok: false,
        error: `lifecycle complete but an extra agent-terminal Bash call followed: ${ev.command}`,
      }
    }
    const milestone = milestones[step]
    if (!milestone.match(ev.command)) {
      if (mode === "real" && isObservational(ev.command)) {
        // Real models legitimately poll with list/read between lifecycle steps.
        continue
      }
      return {
        ok: false,
        error: `out-of-order or extraneous agent-terminal Bash call at milestone ${step + 1}: ${ev.command}`,
      }
    }
    attempts[step]++
    const result = milestone.check(ev.output)
    if (!result.ok) {
      if (attempts[step] >= maxAttempts) {
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

  let successSeen = false
  for (let i = events.length - 1; i >= 0; i--) {
    const ev = events[i]
    if (ev.type === "text" && isTextPart(ev.part) && ev.part.text) {
      const lines = ev.part.text.split(/\r?\n/)
      if (lines.some((line) => line.trim() === "E2E_SUCCESS")) {
        successSeen = true
        break
      }
    }
  }
  if (!successSeen) {
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
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--mode" && i + 1 < args.length) {
      const v = args[i + 1]
      mode = v === "real" ? "real" : "strict"
      i++
      continue
    }
    if (!path) path = args[i]
    else if (!jobName) jobName = args[i]
  }
  if (!path || !jobName) {
    console.error("usage: bun run e2e-verify.ts <transcript.jsonl> <job-name> [--mode strict|real]")
    process.exit(2)
  }
  const result = verifyE2E(path, jobName, mode)
  if (!result.ok) {
    console.error(`e2e verification failed: ${result.error}`)
    process.exit(1)
  }
  console.log("e2e verification passed")
}

main()
