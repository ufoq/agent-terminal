import { statSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import {
  getAgentDir,
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

/**
 * Build the load-time PATH with the requested ordering:
 * bundled dirs first (zellij before agent-terminal), then the pi managed bin
 * dir, then the pre-existing entries, deduplicated. Inserting the managed bin
 * explicitly means the SDK's getShellEnv `hasBinDir` check passes, so it never
 * prepends a second copy and the bundled binaries keep precedence.
 */
function computePackagePathEnv(
  packageRoot: string,
  managedBinDir: string,
  basePath: string,
  bundledZellijMissing: boolean,
): string {
  let path = prependPathEntry(managedBinDir, basePath)
  path = prependPathEntry(join(packageRoot, "bin", "linux-x64"), path)
  return bundledZellijMissing ? path : prependPathEntry(join(packageRoot, "bin", "zellij"), path)
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

    // Compatibility flags for the shared agent-terminal harness. pi has no
    // native --cwd/--no-lsp flags, so these are accepted and described
    // honestly without claiming behavior.
    pi.registerFlag("cwd", {
      type: "string",
      description:
        "Accepted for compatibility with the agent-terminal harness; the bash tool's working directory is the invocation directory.",
    })
    pi.registerFlag("no-lsp", {
      type: "boolean",
      description:
        "Accepted for compatibility with the agent-terminal harness; pi has no LSP integration to disable.",
    })

    const agentTerminalBin = join(packageRoot, "bin", "linux-x64", "agent-terminal")
    if (!isExecutableFile(agentTerminalBin)) {
      stderr(
        `[agent-terminal] bundled executable is missing or not executable: ${agentTerminalBin}`,
      )
      return
    }

    const zellijBin = join(packageRoot, "bin", "zellij", "zellij")
    const bundledZellijMissing = !isExecutableFile(zellijBin)

    // Expose the bundled binaries to every child process. pi's bash tool
    // live-spreads process.env (getShellEnv), so a load-time PATH mutation is
    // visible to the stock bash tool — no bash hook needed.
    process.env["PATH"] = computePackagePathEnv(
      packageRoot,
      join(getAgentDir(), "bin"),
      process.env["PATH"] ?? "",
      bundledZellijMissing,
    )

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
