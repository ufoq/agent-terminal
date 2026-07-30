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

function selectedBinaryDir(packageRoot: string, arch: string): string | null {
  switch (arch) {
    case "x64":
      return join(packageRoot, "bin", "linux-x64")
    case "arm64":
      return join(packageRoot, "bin", "linux-arm64")
    default:
      return null
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

export async function createServerHooks(input: CreateServerHooksInput = {}): Promise<Hooks> {
  const platform = input.platform ?? process.platform
  const arch = input.arch ?? process.arch
  const packageRoot = input.packageRoot ?? DEFAULT_PACKAGE_ROOT
  const stderr = input.stderr ?? writeDiagnostic

  if (platform !== "linux") {
    stderr(`[agent-terminal] unsupported platform: ${platform}. This package supports Linux only.`)
    return {}
  }

  const binDir = selectedBinaryDir(packageRoot, arch)
  if (binDir === null) {
    stderr(`[agent-terminal] unsupported architecture: ${arch}. Expected x64 or arm64.`)
    return {}
  }

  const executable = join(binDir, "agent-terminal")
  if (!isExecutableFile(executable)) {
    stderr(`[agent-terminal] bundled executable is missing or not executable: ${executable}`)
    return {}
  }

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
      output.env["PATH"] = prependPathEntry(binDir, output.env["PATH"] ?? process.env["PATH"] ?? "")
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
