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
  readonly sessionID?: string
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

// The scope probe is the first Bash call in the deterministic gate. Its output
// is the OpenCode session id the plugin's shell.env hook injected.
const SESSION_ID = "ses_12345"
const SCOPE_PROBE_OUTPUT = `${SESSION_ID}\n`

function scopeProbe(outputText: string = SCOPE_PROBE_OUTPUT, sessionID?: string): TranscriptEvent {
  return {
    type: "tool_use",
    ...(sessionID === undefined ? {} : { sessionID }),
    part: {
      type: "tool",
      tool: "bash",
      callID: "scope-probe",
      state: {
        status: "completed",
        input: { command: "printenv AGENT_TERMINAL_SCOPE" },
        output: outputText,
      },
    },
  }
}

const jobName = "prompt-smoke-test"
const startCommand =
  'agent-terminal start prompt-smoke-test -- /bin/bash -lc \'printf "prompt-ready\\n"; IFS= read -r first; printf "first:%s\\n" "$first"; IFS= read -r second; printf "second:%s\\n" "$second"\''

function output(fields: Readonly<Record<string, unknown>>): string {
  return JSON.stringify({ status: "ok", ...fields })
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
    scopeProbe(),
    tool("list-1", "agent-terminal list", output({ jobs: [] })),
    tool("start-1", startCommand, output({ state: "running" })),
    ...extra,
    tool(
      "read-1",
      "agent-terminal read prompt-smoke-test",
      output({ state: "running", screen: "prompt-ready", truncated: false }),
    ),
    tool("send-1", "agent-terminal send prompt-smoke-test -- hello-e2e", output({})),
    tool(
      "read-2",
      "agent-terminal read prompt-smoke-test",
      output({ state: "running", screen: "first:hello-e2e", truncated: false }),
    ),
    tool("press-1", "agent-terminal press prompt-smoke-test -- Enter", output({})),
    tool(
      "read-3",
      "agent-terminal read prompt-smoke-test",
      output({ state: "exited", exit_code: 0, screen: "second:", truncated: false }),
    ),
    tool("stop-1", "agent-terminal stop prompt-smoke-test", output({})),
    tool("list-2", "agent-terminal list", output({ jobs: [] })),
    { type: "text", part: { type: "text", text: "E2E_SUCCESS" } },
  ]
}

async function verifyEvents(
  events: readonly TranscriptEvent[],
  mode: "strict" | "real",
  expectedWorkdir?: string,
  stampSessionID = true,
) {
  const directory = await mkdtemp(join(tmpdir(), "agent-terminal-verify-"))
  const transcript = join(directory, "transcript.jsonl")
  try {
    // Mirror the real `opencode run` transcript: every event line carries the
    // same top-level sessionID.
    const stamped = stampSessionID
      ? events.map((event) => ({ ...event, sessionID: event.sessionID ?? SESSION_ID }))
      : events
    await writeFile(transcript, `${stamped.map((event) => JSON.stringify(event)).join("\n")}\n`)
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
          output({ state: "running", screen: "starting", truncated: false }),
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
        output({ state: "running", screen: "starting", truncated: false }),
      ),
    )
    const result = await verifyEvents(successfulLifecycle(extraReads), "real")

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("failed after 8 attempts")
  })

  it("rejects compound agent-terminal mutations", async () => {
    const result = await verifyEvents(
      [
        scopeProbe(),
        tool("compound", "agent-terminal stop prompt-smoke-test; agent-terminal list", output({})),
      ],
      "real",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("invalid agent-terminal command")
  })

  it("rejects duplicate mutations even in real mode", async () => {
    const result = await verifyEvents(
      successfulLifecycle([tool("duplicate-start", startCommand, output({ state: "running" }))]),
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
            '{"status":"error","code":"job_not_running","message":"job is not running"}',
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
      [scopeProbe(), tool("list-1", "agent-terminal list", output({ jobs: [] }), { exit: 1 })],
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
        [scopeProbe(), tool(`invalid-${index}`, command, output({ jobs: [] }))],
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
      [
        scopeProbe(),
        tool("list-1", "agent-terminal list", output({ jobs: [] }), undefined, "/tmp/external"),
      ],
      "real",
      "/tmp/harness",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("unexpected workdir")
  })
})

describe("scope-probe verification", () => {
  it("requires the scope probe to be present", async () => {
    const result = await verifyEvents(successfulLifecycle().slice(1), "strict")

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("scope probe Bash call")
  })

  it("rejects a scope value that does not match the transcript session id", async () => {
    const result = await verifyEvents(
      [scopeProbe("ses_99999\n"), ...successfulLifecycle().slice(1)],
      "strict",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("does not match the OpenCode session id")
  })

  it("rejects an empty scope value", async () => {
    const result = await verifyEvents(
      [scopeProbe("\n"), ...successfulLifecycle().slice(1)],
      "strict",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("does not match the OpenCode session id")
  })

  it("requires a sessionID in strict mode", async () => {
    const result = await verifyEvents(
      [scopeProbe(SCOPE_PROBE_OUTPUT), ...successfulLifecycle().slice(1)],
      "strict",
      undefined,
      false,
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("transcript does not carry a sessionID")
  })

  it("accepts a matching scope value in strict mode", async () => {
    const result = await verifyEvents(successfulLifecycle(), "strict")

    expect(result).toEqual({ ok: true })
  })

  it("requires the probe to be the first Bash call", async () => {
    const result = await verifyEvents(
      [
        tool("list-1", "agent-terminal list", output({ jobs: [] })),
        scopeProbe(),
        ...successfulLifecycle().slice(2),
      ],
      "strict",
    )

    expect(result.ok).toBe(false)
    if (!result.ok) expect(result.error).toContain("must be the first Bash call")
  })
})
