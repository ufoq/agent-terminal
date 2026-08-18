import { statSync } from "node:fs"
import { basename, dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import {
  createBashTool,
  type BashSpawnContext,
  type ExtensionAPI,
  type ExtensionFactory,
} from "@earendil-works/pi-coding-agent"

type CreateExtensionInput = {
  readonly arch?: string
  readonly packageRoot?: string
  readonly platform?: string
  readonly stderr?: (message: string) => void
}

const DEFAULT_PACKAGE_ROOT = dirname(dirname(fileURLToPath(import.meta.url)))

const SCOPE_ENV = "AGENT_TERMINAL_SCOPE"
const PI_SESSION_ID_ENV = "PI_SESSION_ID"
const PI_SESSION_FILE_ENV = "PI_SESSION_FILE"

function isExecutableFile(path: string): boolean {
  const stat = statSync(path, { throwIfNoEntry: false })
  return stat?.isFile() === true && (stat.mode & 0o111) !== 0
}

function writeDiagnostic(message: string): void {
  process.stderr.write(`${message}\n`)
}

function prependPathEntry(entry: string, path: string): string {
  const entries = path.split(":").filter((candidate) => candidate.length > 0 && candidate !== entry)
  return [entry, ...entries].join(":")
}

function agentTerminalBinDir(packageRoot: string): string {
  return join(packageRoot, "bin", "linux-x64")
}

function computePackagePathEnv(
  packageRoot: string,
  basePath: string,
  bundledZellijMissing: boolean,
): string {
  const agentTerminalDir = agentTerminalBinDir(packageRoot)
  const zellijDir = join(packageRoot, "bin", "zellij")
  const path = prependPathEntry(agentTerminalDir, basePath)
  return bundledZellijMissing ? path : prependPathEntry(zellijDir, path)
}

/**
 * Derive the pi/omp session id from the environment.
 *
 * pi injects `PI_SESSION_ID` directly. omp injects `PI_SESSION_FILE` — a
 * path like `.../<timestamp>_<sessionId>.jsonl` — so we extract the session id
 * from the filename (the segment after the last underscore, minus the
 * extension).
 */
export function deriveSessionId(env: NodeJS.ProcessEnv): string | null {
  const explicit = env[SCOPE_ENV]
  if (explicit !== undefined && explicit.trim() !== "") {
    return explicit.trim()
  }

  const sessionId = env[PI_SESSION_ID_ENV]
  if (sessionId !== undefined && sessionId.trim() !== "") {
    return sessionId.trim()
  }

  const sessionFile = env[PI_SESSION_FILE_ENV]
  if (sessionFile !== undefined && sessionFile.trim() !== "") {
    const stem = basename(sessionFile.trim()).replace(/\.jsonl$/, "")
    const parts = stem.split("_")
    const derived = parts[parts.length - 1]
    if (derived !== undefined && derived !== "") {
      return derived
    }
  }

  return null
}

/**
 * Build the spawnHook that injects `AGENT_TERMINAL_SCOPE` and prepends the
 * bundled binary directories to PATH for every bash command the agent runs.
 */
export function createSpawnHook(packageRoot: string, bundledZellijMissing: boolean) {
  return (context: BashSpawnContext): BashSpawnContext => {
    const sessionId = deriveSessionId(context.env)
    if (sessionId !== null) {
      const existing = context.env[SCOPE_ENV]
      if (existing === undefined || existing.trim() === "") {
        context.env[SCOPE_ENV] = sessionId
      }
    }

    context.env["PATH"] = computePackagePathEnv(
      packageRoot,
      context.env["PATH"] ?? process.env["PATH"] ?? "",
      bundledZellijMissing,
    )

    return context
  }
}

export const createExtension = (input: CreateExtensionInput = {}): ExtensionFactory => {
  const platform = input.platform ?? process.platform
  const arch = input.arch ?? process.arch
  const packageRoot = input.packageRoot ?? DEFAULT_PACKAGE_ROOT
  const stderr = input.stderr ?? writeDiagnostic

  return (pi: ExtensionAPI): void => {
    if (platform !== "linux") {
      stderr(
        `[agent-terminal] unsupported platform: ${platform}. This package supports Linux only.`,
      )
      return
    }

    if (arch !== "x64") {
      stderr(
        `[agent-terminal] unsupported architecture: ${arch}. This package supports x86_64 Linux only.`,
      )
      return
    }

    // Register CLI flags the harness passes that aren't native to pi.
    // These must be registered before any early return so the flags are
    // always accepted regardless of whether the bundled binary is present.
    pi.registerFlag("cwd", { type: "string", description: "Working directory for the bash tool" })
    pi.registerFlag("no-lsp", { type: "boolean", description: "Disable LSP integration" })
    pi.registerFlag("no-context-files", {
      type: "boolean",
      description: "Disable context file loading",
    })

    const agentTerminalBin = join(agentTerminalBinDir(packageRoot), "agent-terminal")
    if (!isExecutableFile(agentTerminalBin)) {
      stderr(
        `[agent-terminal] bundled executable is missing or not executable: ${agentTerminalBin}`,
      )
      return
    }

    const zellijBin = join(packageRoot, "bin", "zellij", "zellij")
    const bundledZellijMissing = !isExecutableFile(zellijBin)

    // Dual-exposure: mutate process.env.PATH at load time so the bundled
    // binaries are visible to all child processes (not just the bash tool).
    process.env["PATH"] = computePackagePathEnv(
      packageRoot,
      process.env["PATH"] ?? "",
      bundledZellijMissing,
    )

    // Re-register the bash tool with a spawnHook that injects the per-session
    // scope and ensures the bundled binaries are on PATH inside bash commands.
    const cwdFlag = pi.getFlag("cwd")
    const bashCwd = typeof cwdFlag === "string" && cwdFlag !== "" ? cwdFlag : process.cwd()

    // omp does not inject PI_SESSION_ID / PI_SESSION_FILE into the bash env
    // (even with exposeSessionEnvironment), so the spawnHook can't rely on
    // those vars alone. The execute wrapper captures the session id from the
    // runtime context (ctx.sessionManager.getSessionId()) and stores it here
    // for the spawnHook to read as a fallback.
    let ctxSessionId: string | null = null

    const spawnHook = (context: BashSpawnContext): BashSpawnContext => {
      // Copy env so the injected scope does not leak into process.env and
      // persist across session switches (omp passes process.env directly).
      const env: NodeJS.ProcessEnv = { ...context.env }
      const sessionId = deriveSessionId(env) ?? ctxSessionId
      if (sessionId !== null) {
        const existing = env[SCOPE_ENV]
        if (existing === undefined || existing.trim() === "") {
          env[SCOPE_ENV] = sessionId
        }
      }
      env["PATH"] = computePackagePathEnv(
        packageRoot,
        env["PATH"] ?? process.env["PATH"] ?? "",
        bundledZellijMissing,
      )
      return { command: context.command, cwd: context.cwd, env }
    }

    const bashTool = createBashTool(bashCwd, {
      spawnHook,
      exposeSessionEnvironment: true,
    })

    type BashTool = typeof bashTool
    type BashParams = Parameters<BashTool["execute"]>[1]
    type BashUpdateCallback = Parameters<BashTool["execute"]>[3]
    type BashResult = Awaited<ReturnType<BashTool["execute"]>>

    pi.registerTool({
      ...bashTool,
      execute: async (
        toolCallId: string,
        params: BashParams,
        signal?: AbortSignal,
        onUpdate?: BashUpdateCallback,
        ctx?: unknown,
      ): Promise<BashResult> => {
        // Capture the session id from the runtime context for omp, which
        // doesn't inject PI_SESSION_ID into the bash tool env.
        if (ctx !== null && typeof ctx === "object" && "sessionManager" in ctx) {
          const sm = (ctx as { sessionManager: { getSessionId?: () => string } }).sessionManager
          if (typeof sm?.getSessionId === "function") {
            const id = sm.getSessionId()
            if (id !== undefined && id !== "") {
              ctxSessionId = id
            }
          }
        }
        return (bashTool.execute as (...args: unknown[]) => Promise<BashResult>)(
          toolCallId,
          params,
          signal,
          onUpdate,
          ctx,
        )
      },
    })

    // Register the agent-terminal skill directory.
    pi.on("resources_discover", async () => {
      return {
        skillPaths: [join(packageRoot, "skills")],
      }
    })
  }
}

const defaultExtension = createExtension()

// biome-ignore lint/style/noDefaultExport: pi's extension loader detects extensions via default export.
export default defaultExtension
