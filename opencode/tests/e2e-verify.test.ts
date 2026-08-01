import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { describe, expect, it } from "bun:test"
import { verifyE2E } from "../scripts/e2e-verify"

type TranscriptTool = {
  readonly type: "tool"
  readonly tool: string
  readonly callID: string
  readonly state: {
    readonly status: string
    readonly input?: { readonly command?: string; readonly workdir?: string }
    readonly output?: string
    readonly error?: string
    readonly metadata?: Readonly<Record<string, unknown>>
  }
}

type TranscriptText = {
  readonly type: "text"
  readonly text: string
}

type TranscriptEvent = {
  readonly type: "tool_use" | "text"
  readonly part: TranscriptTool | TranscriptText
}

function failedTool(callID: string): TranscriptEvent {
  return {
    type: "tool_use",
    part: {
      type: "tool",
      tool: "task",
      callID,
      state: { status: "error", error: "tool failed" },
    },
  }
}

const jobName = "prompt-smoke-test"
const startCommand =
  'agent-terminal start prompt-smoke-test -- /bin/bash -lc \'printf "prompt-ready\\n"; IFS= read -r first; printf "first:%s\\n" "$first"; IFS= read -r second; printf "second:%s\\n" "$second"\''

function output(data: Readonly<Record<string, unknown>>): string {
  return JSON.stringify({ status: "ok", data })
}

function tool(
  callID: string,
  command: string,
  result: string,
  metadata?: Readonly<Record<string, unknown>>,
  workdir?: string,
): TranscriptEvent {
  return {
    type: "tool_use",
    part: {
      type: "tool",
      tool: "bash",
      callID,
      state: {
        status: "completed",
        input: { command, ...(workdir === undefined ? {} : { workdir }) },
        output: result,
        ...(metadata === undefined ? {} : { metadata }),
      },
    },
  }
}

function successfulLifecycle(extra: readonly TranscriptEvent[] = []): TranscriptEvent[] {
  return [
    tool("list-1", "agent-terminal list", output({ jobs: [] })),
    tool("start-1", startCommand, output({ job: jobName, state: "running" })),
    ...extra,
    tool(
      "read-1",
      "agent-terminal read prompt-smoke-test",
      output({ job: jobName, state: "running", screen: "prompt-ready" }),
    ),
    tool(
      "send-1",
      "agent-terminal send prompt-smoke-test -- hello-e2e",
      output({ job: jobName, issued: "text", submitted: true }),
    ),
    tool(
      "read-2",
      "agent-terminal read prompt-smoke-test",
      output({ job: jobName, state: "running", screen: "first:hello-e2e" }),
    ),
    tool(
      "press-1",
      "agent-terminal press prompt-smoke-test -- Enter",
      output({ job: jobName, issued: "keys", keys: ["Enter"] }),
    ),
    tool(
      "read-3",
      "agent-terminal read prompt-smoke-test",
      output({ job: jobName, state: "exited", exit_code: 0, screen: "second:" }),
    ),
    tool(
      "stop-1",
      "agent-terminal stop prompt-smoke-test",
      output({ job: jobName, cleaned_up: true }),
    ),
    tool("list-2", "agent-terminal list", output({ jobs: [] })),
    { type: "text", part: { type: "text", text: "E2E_SUCCESS" } },
  ]
}

async function verifyEvents(
  events: readonly TranscriptEvent[],
  mode: "strict" | "real",
  expectedWorkdir?: string,
) {
  const directory = await mkdtemp(join(tmpdir(), "agent-terminal-verify-"))
  const transcript = join(directory, "transcript.jsonl")
  try {
    await writeFile(transcript, `${events.map((event) => JSON.stringify(event)).join("\n")}\n`)
    return verifyE2E(transcript, jobName, mode, expectedWorkdir)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
}

describe("real-model transcript verification", () => {
  it("accepts bounded exact read retries at the current milestone", async () => {
    const result = await verifyEvents(
      successfulLifecycle([
        tool(
          "read-extra",
          "agent-terminal read prompt-smoke-test",
          output({ job: jobName, state: "running", screen: "starting" }),
        ),
      ]),
      "real",
    )

    expect(result).toEqual({ ok: true })
  })

  it("bounds observational calls in real mode", async () => {
    const extraReads = Array.from({ length: 9 }, (_, index) =>
      tool(
        `read-extra-${index}`,
        "agent-terminal read prompt-smoke-test",
        output({ job: jobName, state: "running", screen: "starting" }),
      ),
    )
    const result = await verifyEvents(successfulLifecycle(extraReads), "real")

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("failed after 8 attempts")
  })

  it("rejects compound agent-terminal mutations", async () => {
    const result = await verifyEvents(
      [
        tool(
          "compound",
          "agent-terminal stop prompt-smoke-test; agent-terminal list",
          output({ job: jobName, cleaned_up: true }),
        ),
      ],
      "real",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("invalid agent-terminal command")
  })

  it("rejects duplicate mutations even in real mode", async () => {
    const result = await verifyEvents(
      successfulLifecycle([
        tool("duplicate-start", startCommand, output({ job: jobName, state: "running" })),
      ]),
      "real",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("out-of-order")
  })

  it("rejects failed mutation results instead of retrying them", async () => {
    const result = await verifyEvents(
      successfulLifecycle([]).map((event) => {
        if (
          event.type === "tool_use" &&
          event.part.type === "tool" &&
          event.part.callID === "send-1"
        ) {
          return tool(
            "send-1",
            "agent-terminal send prompt-smoke-test -- hello-e2e",
            '{"status":"error"}',
          )
        }
        return event
      }),
      "real",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain('milestone "send" failed')
  })

  it("rejects nonzero Bash metadata before lifecycle verification", async () => {
    const result = await verifyEvents(
      [tool("list-1", "agent-terminal list", output({ jobs: [] }), { exit: 1 })],
      "real",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("non-zero exit status")
  })

  it("rejects prefixed and wrapped agent-terminal mutations", async () => {
    const commands = [
      ":; agent-terminal stop prompt-smoke-test",
      "command agent-terminal stop prompt-smoke-test",
      "env SAFE=1 agent-terminal stop prompt-smoke-test",
      'agent-terminal list --project "$(pwd; agent-terminal stop prompt-smoke-test)"',
      "agent-terminal start prompt-smoke-test -- /bin/bash -lc 'agent-terminal stop prompt-smoke-test'",
      "a\\gent-terminal stop prompt-smoke-test",
    ]

    for (const [index, command] of commands.entries()) {
      const result = await verifyEvents(
        [tool(`invalid-${index}`, command, output({ jobs: [] }))],
        "real",
      )

      expect(result.ok).toBe(false)
      if (!result.ok) {
        expect(
          result.error.includes("invalid agent-terminal command") ||
            result.error.includes("not an agent-terminal lifecycle command"),
        ).toBe(true)
      }
    }
  })

  it("rejects failed unrelated tools before real-mode filtering", async () => {
    const result = await verifyEvents([failedTool("task-1"), ...successfulLifecycle()], "real")

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("tool call failed")
  })

  it("rejects successful unrelated tools instead of treating them as evidence-free", async () => {
    const result = await verifyEvents(
      [
        {
          type: "tool_use",
          part: {
            type: "tool",
            tool: "task",
            callID: "task-1",
            state: { status: "completed", output: "delegated" },
          },
        },
        ...successfulLifecycle(),
      ],
      "real",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("non-Bash tool call")
  })

  it("rejects an external Bash workdir", async () => {
    const result = await verifyEvents(
      [tool("list-1", "agent-terminal list", output({ jobs: [] }), undefined, "/tmp/external")],
      "real",
      "/tmp/harness",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("unexpected workdir")
  })
})
