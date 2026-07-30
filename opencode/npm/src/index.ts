import { statSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

type SkillConfig = {
  skills?: {
    paths?: string[]
  }
}

type ShellEnvOutput = {
  env: Record<string, string | undefined>
}

export type Hooks = {
  readonly config?: (config: SkillConfig) => void
  readonly "shell.env"?: (input: unknown, output: ShellEnvOutput) => void
}

export type PluginModule = {
  readonly id: string
  readonly server: (input?: CreateServerHooksInput) => Promise<Hooks>
}

export type CreateServerHooksInput = {
  readonly arch?: string
  readonly packageRoot?: string
  readonly platform?: string
  readonly stderr?: (message: string) => void
}

const DEFAULT_PACKAGE_ROOT = dirname(dirname(fileURLToPath(import.meta.url)))

function selectedAgentTerminalDir(packageRoot: string, arch: string): string | null {
  if (arch === "x64") {
    return join(packageRoot, "bin", "linux-x64")
  }
  return null
}

function bundledZellijDir(packageRoot: string): string {
  return join(packageRoot, "bin", "zellij")
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

function computePackagePathEnv(
  packageRoot: string,
  basePath: string,
  bundledZellijMissing: boolean,
): string {
  const agentTerminalDir = join(packageRoot, "bin", "linux-x64")
  const zellijDir = bundledZellijDir(packageRoot)

  const path = prependPathEntry(agentTerminalDir, basePath)
  return bundledZellijMissing ? path : prependPathEntry(zellijDir, path)
}

export async function createServerHooks(input: CreateServerHooksInput = {}): Promise<Hooks> {
  const platform = input.platform ?? process.platform
  const arch = input.arch ?? process.arch
  const packageRoot = input.packageRoot ?? DEFAULT_PACKAGE_ROOT
  const stderr = input.stderr ?? writeDiagnostic

  if (platform !== "linux") {
    stderr(`[agent-terminal] unsupported platform: ${platform}. This package supports Linux only.`)
    return {}
  }

  const agentTerminalDir = selectedAgentTerminalDir(packageRoot, arch)
  if (agentTerminalDir === null) {
    stderr(
      `[agent-terminal] unsupported architecture: ${arch}. This package supports x86_64 Linux only.`,
    )
    return {}
  }

  const agentTerminalBin = join(agentTerminalDir, "agent-terminal")
  if (!isExecutableFile(agentTerminalBin)) {
    stderr(`[agent-terminal] bundled executable is missing or not executable: ${agentTerminalBin}`)
    return {}
  }

  const zellijDir = bundledZellijDir(packageRoot)
  const zellijBin = join(zellijDir, "zellij")
  const bundledZellijMissing = !isExecutableFile(zellijBin)

  process.env["PATH"] = computePackagePathEnv(
    packageRoot,
    process.env["PATH"] ?? "",
    bundledZellijMissing,
  )

  const skillsPath = join(packageRoot, "skills")

  return {
    config: (config) => {
      const skills = config.skills ?? {}
      const paths = skills.paths ?? []
      if (paths.includes(skillsPath)) {
        config.skills = { ...skills, paths }
        return
      }
      config.skills = { ...skills, paths: [...paths, skillsPath] }
    },
    "shell.env": (_input, output) => {
      output.env["PATH"] = computePackagePathEnv(
        packageRoot,
        output.env["PATH"] ?? process.env["PATH"] ?? "",
        bundledZellijMissing,
      )
    },
  }
}

export const server = createServerHooks

export const plugin: PluginModule = {
  id: "opencode-agent-terminal",
  server,
}

// biome-ignore lint/style/noDefaultExport: OpenCode's V1 plugin loader detects package plugins via a default plugin object.
export default plugin
