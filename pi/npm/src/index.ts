import { execFileSync } from "node:child_process"
import { readFileSync, statSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import {
  type ExtensionAPI,
  type ExtensionFactory,
  getAgentDir,
} from "@earendil-works/pi-coding-agent"

type CreateExtensionInput = {
  readonly arch?: string
  readonly packageRoot?: string
  readonly platform?: string
  readonly stderr?: (message: string) => void
}

const DEFAULT_PACKAGE_ROOT = dirname(dirname(fileURLToPath(import.meta.url)))

type PackageManifest = {
  readonly agentTerminal?: { readonly zellijVersion?: unknown }
}

function bundledZellijVersion(packageRoot: string): string | undefined {
  try {
    const raw = readFileSync(join(packageRoot, "package.json"), "utf8")
    const manifest = JSON.parse(raw) as PackageManifest
    const version = manifest.agentTerminal?.zellijVersion
    return typeof version === "string" ? version : undefined
  } catch {
    return undefined
  }
}

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
    let useBundledZellij = isExecutableFile(zellijBin)
    const expectedZellijVersion = bundledZellijVersion(packageRoot)
    if (expectedZellijVersion !== undefined) {
      if (!useBundledZellij) {
        stderr(
          `[agent-terminal] warning: bundled Zellij is missing or not executable: ${zellijBin}; continuing without it`,
        )
      } else {
        let detected = ""
        try {
          detected = execFileSync(zellijBin, ["--version"], {
            encoding: "utf8",
            stdio: ["ignore", "pipe", "pipe"],
          }).trim()
        } catch (error) {
          useBundledZellij = false
          stderr(
            `[agent-terminal] warning: bundled Zellij failed to report its version: ${String(error)}; continuing without it`,
          )
        }
        const expected = `zellij ${expectedZellijVersion}`
        if (detected !== "" && detected !== expected) {
          stderr(
            `[agent-terminal] warning: bundled Zellij version mismatch: expected ${expected}, got ${JSON.stringify(detected)}; continuing anyway`,
          )
        }
      }
    }
    const bundledZellijMissing = !useBundledZellij

    // Expose the bundled binaries to every child process. pi's bash tool
    // live-spreads process.env (getShellEnv), so a load-time PATH mutation is
    // visible to the stock bash tool — no bash hook needed.
    process.env["PATH"] = computePackagePathEnv(
      packageRoot,
      join(getAgentDir(), "bin"),
      process.env["PATH"] ?? "",
      bundledZellijMissing,
    )
  }
}

const defaultExtension = createExtension()

// biome-ignore lint/style/noDefaultExport: pi's extension loader detects extensions via default export.
export default defaultExtension
