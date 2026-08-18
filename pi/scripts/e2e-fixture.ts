// Deterministic OpenAI-compatible e2e fixture for pi's openai-completions API.
// Drives the 9-step agent-terminal lifecycle by parsing incoming request
// history and emitting the next genuine Bash tool_call. A step only advances
// after BOTH the assistant tool call and its validated tool result are seen;
// read steps whose marker has not rendered yet are retried (bounded).
//
// pi sends message content as ARRAYS ([{type:"text",text:"..."}]) rather than
// plain strings, so every content reader handles both forms. Responses are
// always streamed as SSE chunks (pi's openai-completions parser requires
// streaming; a single non-delta JSON response fails with "Stream ended
// without finish_reason").
//
// Run: bun run pi/scripts/e2e-fixture.ts [--port PORT]

const portIdx = Bun.argv.indexOf("--port")
const portValue = portIdx >= 0 ? Bun.argv[portIdx + 1] : undefined
const PORT = portValue ? parseInt(portValue, 10) : 18990

const MAX_READ_RETRIES = 5

type StepCheck = { ok: true } | { ok: false; retry: boolean; error: string }

// pi sends message content as an array of content parts
// ([{type:"text",text:"..."}]) rather than a plain string; both forms occur
// in request history, so every content reader handles the union.
type ContentPart = { type: string; text: string }

type MessageContent = string | ContentPart[] | null

type ToolResult = {
  role: string
  content: MessageContent
  tool_call_id?: string
}

type ToolCall = {
  id?: string
  function: { name: string; arguments: string }
}

type HistoryMessage = {
  role: string
  content: MessageContent
  tool_calls?: ToolCall[]
} & ToolResult

const SCOPE_PROBE_COMMAND = "printenv AGENT_TERMINAL_SCOPE"

const STEPS = [
  {
    // First Bash call: print the plugin-injected scope. Its result is raw
    // text (the pi session id), not JSON; only non-empty is required here,
    // the verifier asserts the exact session-id match.
    probe: true as const,
    cmd: (_job: string) => SCOPE_PROBE_COMMAND,
    onResult: "scope-probe",
  },
  {
    cmd: (_job: string) => `agent-terminal list`,
    onResult: "list",
    check: (body: Record<string, unknown>): StepCheck => {
      if (!Array.isArray(body["jobs"]))
        return {
          ok: false,
          retry: false,
          error: `jobs is not an array: ${JSON.stringify(body["jobs"])}`,
        }
      if (body["jobs"].length !== 0)
        return {
          ok: false,
          retry: false,
          error: `expected empty jobs, got ${JSON.stringify(body["jobs"])}`,
        }
      return { ok: true }
    },
  },
  {
    cmd: (job: string) =>
      `agent-terminal start ${job} -- /bin/bash -lc 'printf "prompt-ready\\n"; IFS= read -r first; printf "first:%s\\n" "$first"; IFS= read -r second; printf "second:%s\\n" "$second"'`,
    onResult: "start",
    check: (body: Record<string, unknown>, _job: string): StepCheck => {
      if (body["state"] !== "running")
        return {
          ok: false,
          retry: false,
          error: `state != running: ${JSON.stringify(body["state"])}`,
        }
      return { ok: true }
    },
  },
  {
    cmd: (job: string) => `agent-terminal read ${job}`,
    onResult: "read-prompt",
    check: (body: Record<string, unknown>, _job: string): StepCheck => {
      if (typeof body["screen"] !== "string")
        return {
          ok: false,
          retry: false,
          error: `screen is not a string: ${JSON.stringify(body["screen"])}`,
        }
      if (!body["screen"].includes("prompt-ready"))
        return {
          ok: false,
          retry: true,
          error: `screen lacks prompt-ready (state=${JSON.stringify(body["state"])}): ${body["screen"].substring(0, 80)}`,
        }
      return { ok: true }
    },
  },
  {
    cmd: (job: string) => `agent-terminal send ${job} -- hello-e2e`,
    onResult: "send",
    check: (_body: Record<string, unknown>, _job: string): StepCheck => {
      return { ok: true }
    },
  },
  {
    cmd: (job: string) => `agent-terminal read ${job}`,
    onResult: "read-first",
    check: (body: Record<string, unknown>, _job: string): StepCheck => {
      if (typeof body["screen"] !== "string")
        return {
          ok: false,
          retry: false,
          error: `screen is not a string: ${JSON.stringify(body["screen"])}`,
        }
      if (!body["screen"].includes("first:hello-e2e"))
        return {
          ok: false,
          retry: true,
          error: `screen lacks first:hello-e2e (state=${JSON.stringify(body["state"])}): ${body["screen"].substring(0, 80)}`,
        }
      return { ok: true }
    },
  },
  {
    cmd: (job: string) => `agent-terminal press ${job} -- Enter`,
    onResult: "press",
    check: (_body: Record<string, unknown>, _job: string): StepCheck => {
      return { ok: true }
    },
  },
  {
    cmd: (job: string) => `agent-terminal read ${job}`,
    onResult: "read-second",
    check: (body: Record<string, unknown>, _job: string): StepCheck => {
      if (body["state"] !== "exited")
        return {
          ok: false,
          retry: true,
          error: `state != exited: ${JSON.stringify(body["state"])}`,
        }
      if (body["exit_code"] !== 0)
        return {
          ok: false,
          retry: false,
          error: `exit_code != 0: ${JSON.stringify(body["exit_code"])}`,
        }
      if (typeof body["screen"] !== "string" || !body["screen"].includes("second:"))
        return {
          ok: false,
          retry: false,
          error: `screen lacks second: ${JSON.stringify(body["screen"])}`,
        }
      return { ok: true }
    },
  },
  {
    cmd: (job: string) => `agent-terminal stop ${job}`,
    onResult: "stop",
    check: (_body: Record<string, unknown>, _job: string): StepCheck => {
      return { ok: true }
    },
  },
  {
    cmd: (_job: string) => `agent-terminal list`,
    onResult: "list-final",
    check: (body: Record<string, unknown>): StepCheck => {
      if (!Array.isArray(body["jobs"]))
        return {
          ok: false,
          retry: false,
          error: `jobs is not an array: ${JSON.stringify(body["jobs"])}`,
        }
      if (body["jobs"].length !== 0)
        return {
          ok: false,
          retry: false,
          error: `expected empty jobs, got ${JSON.stringify(body["jobs"])}`,
        }
      return { ok: true }
    },
  },
] as const

// pi sends message content as an array of content parts
// ([{type:"text",text:"..."}]) rather than a plain string. Normalize both
// forms to the plain text (or null): for arrays, join the text of every
// text-typed part.
function contentToText(content: MessageContent): string | null {
  if (content === null) return null
  if (typeof content === "string") return content
  const parts: string[] = []
  for (const part of content) {
    if (part.type === "text") parts.push(part.text)
  }
  return parts.length > 0 ? parts.join("") : null
}

function extractJobFromMessages(messages: Array<{ content: MessageContent }>): string | null {
  for (const m of messages) {
    const text = contentToText(m.content)
    if (text) {
      const m_ = text.match(/prompt-smoke-\d+/)
      if (m_) return m_[0]
    }
  }
  return null
}

function isFlatResult(value: unknown): value is { status: string } & Record<string, unknown> {
  return (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    "status" in value &&
    typeof value.status === "string"
  )
}

function parseResultBody(content: string): ({ status: string } & Record<string, unknown>) | null {
  const trimmed = content.trim()
  // omp's bash tool appends "\n\nWall time: X seconds\n\nCommand exited with code N"
  // after the actual output. Extract the first JSON object from the text.
  const jsonStart = trimmed.indexOf("{")
  const jsonEnd = trimmed.indexOf("}", jsonStart >= 0 ? jsonStart : 0)
  if (jsonStart < 0 || jsonEnd < 0) {
    return null
  }
  // Try parsing progressively longer substrings to handle nested JSON objects.
  for (let end = jsonEnd + 1; end <= trimmed.length; end++) {
    const candidate = trimmed.slice(jsonStart, end)
    try {
      const parsed: unknown = JSON.parse(candidate)
      return isFlatResult(parsed) ? parsed : null
    } catch {}
  }
  return null
}

// Parse a tool_call arguments JSON blob and narrow it to the bash command
// string, returning null when the shape does not match.
function parseCommand(argumentsJson: string): string | null {
  try {
    const parsed: unknown = JSON.parse(argumentsJson)
    if (
      typeof parsed === "object" &&
      parsed !== null &&
      !Array.isArray(parsed) &&
      "command" in parsed &&
      typeof parsed.command === "string"
    ) {
      return parsed.command
    }
    return null
  } catch {
    return null
  }
}

function validateStep(stepIdx: number, job: string, content: string | null): StepCheck {
  if (content === null || content === "") {
    return { ok: false, retry: false, error: `step ${stepIdx + 1}: empty tool result` }
  }
  const step = STEPS[stepIdx]
  if (step === undefined) {
    return { ok: false, retry: false, error: `step ${stepIdx + 1}: unknown step index` }
  }
  // The scope probe's result is raw text (the printed session id), not JSON.
  if ("probe" in step) {
    if (content.trim() === "")
      return { ok: false, retry: false, error: "scope probe printed an empty AGENT_TERMINAL_SCOPE" }
    return { ok: true }
  }
  const body = parseResultBody(content)
  if (!body) {
    return {
      ok: false,
      retry: false,
      error: `step ${stepIdx + 1}: non-JSON result: ${content.substring(0, 200)}`,
    }
  }
  if (body.status !== "ok") {
    return {
      ok: false,
      retry: false,
      error: `step ${stepIdx + 1}: status != ok: ${JSON.stringify(body)}`,
    }
  }
  return step.check(body, job)
}

function countCompletedSteps(messages: HistoryMessage[]): {
  count: number
  lastError: string | null
  retryRead: boolean
} {
  const job = extractJobFromMessages(messages)
  if (!job) return { count: 0, lastError: "Could not find job name in messages", retryRead: false }

  let step = 0
  let pendingCall: { stepIdx: number; callId: string | undefined } | null = null
  let retryableStep = -1
  let retryCount = 0

  for (let i = 0; i < messages.length; i++) {
    const m = messages[i]
    if (m === undefined) continue

    if (m.role === "assistant" && m.tool_calls && m.tool_calls.length > 0) {
      if (pendingCall) {
        return {
          count: step,
          lastError: `step ${pendingCall.stepIdx + 1}: tool result missing before next tool call`,
          retryRead: false,
        }
      }
      const tc = m.tool_calls[0]
      if (!tc) continue
      if (tc.function.name !== "bash") {
        return {
          count: step,
          lastError: `expected bash tool, got ${tc.function.name}`,
          retryRead: false,
        }
      }
      const parsedCmd = parseCommand(tc.function.arguments)
      if (parsedCmd === null) {
        return {
          count: step,
          lastError: `step ${step + 1}: could not parse arguments: ${tc.function.arguments}`,
          retryRead: false,
        }
      }
      const actualCmd = parsedCmd
      const expectedStep = STEPS[step]
      if (expectedStep === undefined) {
        return {
          count: step,
          lastError: `step ${step + 1}: unknown step index`,
          retryRead: false,
        }
      }
      const expectedCmd = expectedStep.cmd(job)
      if (actualCmd !== expectedCmd) {
        return {
          count: step,
          lastError: `step ${step + 1}: expected command "${expectedCmd}", got "${actualCmd}"`,
          retryRead: false,
        }
      }
      pendingCall = { stepIdx: step, callId: tc.id }
      continue
    }

    if (m.role === "tool") {
      if (!pendingCall) {
        return {
          count: step,
          lastError: `tool result without a preceding tool call`,
          retryRead: false,
        }
      }
      const result = validateStep(pendingCall.stepIdx, job, contentToText(m.content))
      if (!result.ok) {
        if (result.retry) {
          if (retryableStep === pendingCall.stepIdx) {
            retryCount++
          } else {
            retryableStep = pendingCall.stepIdx
            retryCount = 1
          }
          if (retryCount > MAX_READ_RETRIES) {
            return {
              count: step,
              lastError: `step ${pendingCall.stepIdx + 1}: exceeded ${MAX_READ_RETRIES} read retries: ${result.error}`,
              retryRead: false,
            }
          }
          pendingCall = null
          return { count: step, lastError: null, retryRead: true }
        }
        return {
          count: step,
          lastError: `step ${pendingCall.stepIdx + 1}: ${result.error}`,
          retryRead: false,
        }
      }
      step = pendingCall.stepIdx + 1
      pendingCall = null
      retryableStep = -1
      retryCount = 0
    }
  }

  if (pendingCall) {
    return {
      count: step,
      lastError: `step ${pendingCall.stepIdx + 1}: tool result missing`,
      retryRead: false,
    }
  }
  return { count: step, lastError: null, retryRead: false }
}

function buildToolCallResponse(
  requestId: string,
  model: string,
  job: string,
  stepIdx: number,
): object {
  const step = STEPS[stepIdx]
  if (step === undefined) {
    throw new Error(`buildToolCallResponse: unknown step index ${stepIdx}`)
  }
  const cmd = step.cmd(job)
  return {
    id: `chatcmpl-fixture-${requestId}`,
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message: {
          role: "assistant",
          content: null,
          tool_calls: [
            {
              id: `call_fixture_step${stepIdx + 1}`,
              type: "function" as const,
              function: { name: "bash", arguments: JSON.stringify({ command: cmd }) },
            },
          ],
        },
        finish_reason: "tool_calls" as const,
      },
    ],
    usage: { completion_tokens: 30, prompt_tokens: 100, total_tokens: 130 },
  }
}

function buildErrorResponse(requestId: string, model: string, error: string): object {
  return {
    id: `chatcmpl-fixture-${requestId}`,
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message: { role: "assistant", content: `E2E_FIXTURE_ERROR: ${error}` },
        finish_reason: "stop" as const,
      },
    ],
    usage: { completion_tokens: 10, prompt_tokens: 100, total_tokens: 110 },
  }
}

function buildSuccessResponse(requestId: string, model: string): object {
  return {
    id: `chatcmpl-fixture-${requestId}`,
    object: "chat.completion",
    created: Math.floor(Date.now() / 1000),
    model,
    choices: [
      {
        index: 0,
        message: {
          role: "assistant",
          content: "E2E_SUCCESS\nAll 9 agent-terminal lifecycle steps completed and verified.",
        },
        finish_reason: "stop" as const,
      },
    ],
    usage: { completion_tokens: 15, prompt_tokens: 100, total_tokens: 115 },
  }
}

function toSSEStreaming(response: object): Response {
  const r = response as {
    id: string
    created: number
    model: string
    choices: Array<{
      message: {
        content: string | null
        tool_calls?: Array<{ id: string; function: { name: string; arguments: string } }>
      }
      finish_reason: "stop" | "tool_calls"
    }>
  }
  const { id, created, model } = r
  const choice = r.choices[0]
  if (choice === undefined) {
    throw new Error("toSSEStreaming: response has no choices")
  }
  const msg = choice.message
  const chunks: string[] = []

  const base = { id, object: "chat.completion.chunk", created, model }
  const chunkFor = (delta: object, finishReason: string | null) =>
    JSON.stringify({ ...base, choices: [{ index: 0, delta, finish_reason: finishReason }] })

  if (msg.tool_calls && msg.tool_calls.length > 0) {
    chunks.push(
      chunkFor(
        {
          role: "assistant",
          content: null,
          tool_calls: msg.tool_calls.map((tc, i) => ({
            index: i,
            id: tc.id,
            type: "function",
            function: { name: tc.function.name, arguments: "" },
          })),
        },
        null,
      ),
    )
    for (let i = 0; i < msg.tool_calls.length; i++) {
      const tc = msg.tool_calls[i]
      if (tc === undefined) continue
      chunks.push(
        chunkFor(
          {
            tool_calls: [{ index: i, function: { arguments: tc.function.arguments } }],
          },
          null,
        ),
      )
    }
    chunks.push(chunkFor({}, "tool_calls"))
  } else {
    chunks.push(chunkFor({ role: "assistant", content: msg.content ?? null }, null))
    chunks.push(chunkFor({}, "stop"))
  }

  const enc = new TextEncoder()
  const stream = new ReadableStream({
    start(controller) {
      for (const c of chunks) {
        controller.enqueue(enc.encode(`data: ${c}\n\n`))
      }
      controller.enqueue(enc.encode("data: [DONE]\n\n"))
      controller.close()
    },
  })
  return new Response(stream, {
    headers: { "content-type": "text/event-stream", "cache-control": "no-cache" },
  })
}

const server = Bun.serve({
  port: PORT,
  hostname: "127.0.0.1",
  async fetch(req) {
    const url = new URL(req.url)

    if (req.method === "GET" && url.pathname === "/v1/models") {
      return Response.json({
        object: "list",
        data: [{ id: "fixture", object: "model", created: 1, owned_by: "fixture" }],
      })
    }

    if (req.method === "GET" && url.pathname === "/health") {
      return new Response("ok", { status: 200, headers: { "content-type": "text/plain" } })
    }

    if (req.method !== "POST" || url.pathname !== "/v1/chat/completions") {
      return new Response("not found", { status: 404 })
    }

    const body: unknown = await req.json()
    const bodyRecord: Record<string, unknown> =
      typeof body === "object" && body !== null && !Array.isArray(body)
        ? (body as Record<string, unknown>)
        : {}
    const rawModel = bodyRecord["model"]
    const model = typeof rawModel === "string" ? rawModel : "fixture"
    const rawMessages = bodyRecord["messages"]
    const messages: HistoryMessage[] = Array.isArray(rawMessages) ? rawMessages : []
    const streaming = bodyRecord["stream"] === true
    const requestId = Math.random().toString(36).slice(2, 10)

    const lastMsg = messages.length > 0 ? messages[messages.length - 1] : null
    console.error(
      `[fixture] request ${requestId}: stream=${streaming} msgs=${messages.length} last_role=${lastMsg?.role || "none"}`,
    )

    const job = extractJobFromMessages(messages)
    if (!job) {
      console.error(`[fixture] request ${requestId}: no job name`)
      const response = buildErrorResponse(requestId, model, "no job name found")
      if (streaming) return toSSEStreaming(response)
      return Response.json(response)
    }

    // Determine progress strictly from validated tool results.
    const { count: completed, lastError, retryRead } = countCompletedSteps(messages)
    if (lastError) {
      console.error(`[fixture] request ${requestId}: error at step ${completed}: ${lastError}`)
      const response = buildErrorResponse(requestId, model, lastError)
      if (streaming) return toSSEStreaming(response)
      return Response.json(response)
    }

    if (completed >= STEPS.length) {
      console.error(`[fixture] request ${requestId}: done! ${STEPS.length} steps completed`)
      const response = buildSuccessResponse(requestId, model)
      if (streaming) return toSSEStreaming(response)
      return Response.json(response)
    }

    // A read marker has not rendered yet: re-emit the same read command.
    if (retryRead) {
      const retryStep = STEPS[completed]
      if (retryStep === undefined) {
        return new Response("fixture: unknown step index", { status: 500 })
      }
      console.error(
        `[fixture] request ${requestId}: step ${completed + 1} → retry ${retryStep.onResult}`,
      )
      const response = buildToolCallResponse(requestId, model, job, completed)
      if (streaming) return toSSEStreaming(response)
      return Response.json(response)
    }

    const nextStep = STEPS[completed]
    if (nextStep === undefined) {
      return new Response("fixture: unknown step index", { status: 500 })
    }
    console.error(`[fixture] request ${requestId}: step ${completed + 1} → ${nextStep.onResult}`)
    const response = buildToolCallResponse(requestId, model, job, completed)
    if (streaming) return toSSEStreaming(response)
    return Response.json(response)
  },
})

console.log(`Fixture listening on http://127.0.0.1:${PORT}`)
process.on("SIGINT", () => {
  server.stop()
  process.exit(0)
})
process.on("SIGTERM" as const, () => {
  server.stop()
  process.exit(0)
})
