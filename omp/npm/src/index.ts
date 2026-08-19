// omp extension adapter for agent-terminal.
//
// Zero runtime imports: the compiled entry is loaded by omp's extension runner
// (an embedded Bun), so everything used here is a runtime global (process, URL,
// import.meta, Bun) — no import declarations and no Node builtin module loads.
// The omp extension surface used by this adapter is declared in the local types
// below; see omp's extension documentation for the authoritative shapes.

type CreateExtensionInput = {
  readonly arch?: string
  readonly packageRoot?: string
  readonly platform?: string
  readonly stderr?: (message: string) => void
}

type FlagOptions = {
  readonly description?: string
  readonly type: "boolean" | "string"
}

type RegisterFlag = (name: string, options: FlagOptions) => void

type ToolCallEvent = {
  readonly input?: Record<string, unknown>
  readonly toolName: string
}

type ToolCallEventResult = {
  readonly block?: boolean
  readonly input?: Record<string, unknown>
  readonly reason?: string
}

type SessionManager = {
  getSessionId(): string
}

type ExtensionContext = {
  readonly sessionManager?: SessionManager
}

type ToolCallHandler = (
  event: ToolCallEvent,
  ctx: ExtensionContext,
) => ToolCallEventResult | undefined

export type ExtensionApi = {
  readonly on: (event: "tool_call", handler: ToolCallHandler) => void
  readonly registerFlag: RegisterFlag
}

type ExtensionFactory = (api: ExtensionApi) => void

// dist/index.js sits at <packageRoot>/dist/index.js. The URL pathname is
// percent-encoded (spaces, non-ASCII, "#", "%"), so it is decoded before use;
// a malformed encoding falls back to the raw pathname. Exported for tests.
export function decodePackageRoot(url: URL): string {
  let pathname = url.pathname
  try {
    pathname = decodeURIComponent(pathname)
  } catch {
    // Invalid percent-encoding: keep the raw pathname.
  }
  return pathname.replace(/\/$/, "")
}

const DEFAULT_PACKAGE_ROOT = decodePackageRoot(new URL("../", import.meta.url))

function joinPath(...parts: string[]): string {
  const joined = parts.join("/").replace(/\/+/g, "/")
  return joined.length > 1 && joined.endsWith("/") ? joined.slice(0, -1) : joined
}

function isExecutableFile(path: string): boolean {
  try {
    // Import-free executable check: `test -f` (regular file) and `test -x`
    // (executable) must both pass — `test -x` alone accepts directories.
    // Spawned via the bun global avoids any Node builtin module load.
    const regularFile = Bun.spawnSync(["test", "-f", path])
    if (regularFile.exitCode !== 0) return false
    const executable = Bun.spawnSync(["test", "-x", path])
    return executable.exitCode === 0
  } catch {
    // `test` unresolvable or spawn failed: treat as not executable.
    return false
  }
}

/**
 * Validate a bash env input: a non-null, non-array record whose values are all
 * strings. Anything else is rejected without revision so omp's native bash
 * schema validation reports it.
 */
function isValidEnv(rawEnv: unknown): rawEnv is Record<string, string> {
  if (rawEnv === null || typeof rawEnv !== "object" || Array.isArray(rawEnv)) return false
  return Object.values(rawEnv).every((value) => typeof value === "string")
}

function writeDiagnostic(message: string): void {
  process.stderr.write(`${message}\n`)
}

function prependPathEntry(entry: string, path: string): string {
  const entries = path.split(":").filter((candidate) => candidate.length > 0 && candidate !== entry)
  return [entry, ...entries].join(":")
}

/**
 * Build the bash env.PATH with the bundled dirs first (zellij before
 * agent-terminal), then the supplied base entries, deduplicated. omp bash
 * children receive only the tool's env input, so this prepend is the only
 * channel that makes the bundled binaries visible to them.
 */
function computeBundledPath(
  packageRoot: string,
  basePath: string,
  bundledZellijMissing: boolean,
): string {
  const path = prependPathEntry(joinPath(packageRoot, "bin", "linux-x64"), basePath)
  return bundledZellijMissing
    ? path
    : prependPathEntry(joinPath(packageRoot, "bin", "zellij"), path)
}

function resolveSessionId(ctx: ExtensionContext): string | undefined {
  const sessionManager = ctx.sessionManager
  if (sessionManager === undefined || sessionManager === null) return undefined
  if (typeof sessionManager.getSessionId !== "function") return undefined
  try {
    const sessionId = sessionManager.getSessionId()
    if (typeof sessionId !== "string" || sessionId.trim() === "") return undefined
    return sessionId
  } catch {
    return undefined
  }
}

export const createExtension = (input: CreateExtensionInput = {}): ExtensionFactory => {
  const platform = input.platform ?? process.platform
  const arch = input.arch ?? process.arch
  const packageRoot = input.packageRoot ?? DEFAULT_PACKAGE_ROOT
  const stderr = input.stderr ?? writeDiagnostic
  let scopeFallbackWarned = false

  return (api: ExtensionApi): void => {
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

    // Compatibility flag for the shared agent-terminal harness: omp has no
    // native --no-context-files flag, so accept it without claiming behavior.
    api.registerFlag("no-context-files", {
      type: "boolean",
      description:
        "Accepted for compatibility with the agent-terminal harness; omp has no context-files feature to disable.",
    })

    const agentTerminalBin = joinPath(packageRoot, "bin", "linux-x64", "agent-terminal")
    if (!isExecutableFile(agentTerminalBin)) {
      stderr(
        `[agent-terminal] bundled executable is missing or not executable: ${agentTerminalBin}`,
      )
      return
    }

    const zellijBin = joinPath(packageRoot, "bin", "zellij", "zellij")
    const bundledZellijMissing = !isExecutableFile(zellijBin)

    // Revise bash tool calls before execution: default AGENT_TERMINAL_SCOPE to
    // the omp session id (never overwriting an explicit value) and prepend the
    // bundled binaries to the bash env.PATH. The native bash tool (schema,
    // approval gate, concurrency, async, PTY) is untouched.
    api.on("tool_call", (event, ctx) => {
      if (event.toolName !== "bash") return

      const input = { ...(event.input ?? {}) }
      const rawEnv = input["env"]
      let env: Record<string, string>
      if (rawEnv === undefined) {
        env = {}
      } else if (isValidEnv(rawEnv)) {
        env = { ...rawEnv }
      } else {
        // Malformed env (null, array, non-string values): leave the call
        // untouched (unrevised) so omp's native bash schema validation
        // reports it.
        return
      }

      const scope = env["AGENT_TERMINAL_SCOPE"]
      if (typeof scope !== "string" || scope.trim() === "") {
        const sessionId = resolveSessionId(ctx)
        if (sessionId !== undefined) {
          env["AGENT_TERMINAL_SCOPE"] = sessionId
        } else if (!scopeFallbackWarned) {
          scopeFallbackWarned = true
          stderr(
            "[agent-terminal] no session id available from omp; AGENT_TERMINAL_SCOPE left unset (agent-terminal falls back to the standalone scope)",
          )
        }
      }

      // `??` keeps an explicit empty-string PATH (bundled dirs only) instead
      // of falling back to the process PATH.
      env["PATH"] = computeBundledPath(
        packageRoot,
        env["PATH"] ?? process.env["PATH"] ?? "",
        bundledZellijMissing,
      )

      return { input: { ...input, env } }
    })
  }
}

const defaultExtension = createExtension()

// biome-ignore lint/style/noDefaultExport: omp's extension loader detects extensions via default export.
export default defaultExtension
