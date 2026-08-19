#!/usr/bin/env node
// Shared build helpers for the agent-terminal npm packages (pi and omp).
//
// The Zellij version + SHA-256 pins live here and ONLY here. Both the pi and
// omp build entrypoints import this library so the pinned binary is verified
// identically everywhere.
//
// The library assumes the repository root is one level above its own directory
// (`scripts/`), so `buildAgentTerminalBinary()` can run cargo from the root and
// `copySkillToPackage()` can copy the single source `opencode/skills`.

import { createHash } from "node:crypto"
import { spawnSync } from "node:child_process"
import {
  chmodSync,
  cpSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  renameSync,
  rmSync,
  statSync,
} from "node:fs"
import { readFile } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { Readable } from "node:stream"
import { finished } from "node:stream/promises"
import { fileURLToPath } from "node:url"

export const ZELLIJ_VERSION = "0.44.3"
export const ZELLIJ_ARCHIVE_SHA256 = "f901129919b0a405ac5f278f53acd7fde5d62401324c509b6233038d5c0ad1f9"
export const ZELLIJ_BINARY_SHA256 = "a675b0106263113b9cb8f028649bad05c5d2283331fa62b2b36dd275aeaaa4d3"

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = dirname(__dirname)

const releaseUrl = `https://github.com/zellij-org/zellij/releases/download/v${ZELLIJ_VERSION}/zellij-no-web-x86_64-unknown-linux-musl.tar.gz`

export function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    stdio: "inherit",
    cwd: options.cwd ?? repoRoot,
    ...options,
  })
  if (result.status !== 0) {
    throw new Error(`Command failed with exit ${result.status ?? "null"}: ${cmd} ${args.join(" ")}`)
  }
  return result
}

export async function sha256File(path) {
  const data = await readFile(path)
  return createHash("sha256").update(data).digest("hex")
}

export function buildAgentTerminalBinary() {
  const targetBin = join(
    repoRoot,
    "target",
    "x86_64-unknown-linux-musl",
    "release",
    "agent-terminal",
  )

  const cargoPath = process.env["CARGO_HOME"]
    ? join(process.env["CARGO_HOME"], "bin", "cargo")
    : "cargo"

  run(cargoPath, ["build", "--release", "--target", "x86_64-unknown-linux-musl"], {
    cwd: repoRoot,
    env: { ...process.env, PATH: `/home/agent/.cargo/bin:${process.env["PATH"] ?? ""}` },
  })

  return targetBin
}

export function copyBinaryToPackage(targetBin, packageRoot) {
  const destDir = join(packageRoot, "bin", "linux-x64")
  const destBin = join(destDir, "agent-terminal")
  mkdirSync(destDir, { recursive: true })
  cpSync(targetBin, destBin, { force: true })
  chmodSync(destBin, 0o755)
}

export function copySkillToPackage(packageRoot) {
  // The skill is shared with the opencode integration (same SKILL.md content),
  // so copy it straight from opencode/skills instead of duplicating it.
  const source = join(repoRoot, "opencode", "skills")
  const dest = join(packageRoot, "skills")
  rmSync(dest, { recursive: true, force: true })
  cpSync(source, dest, { recursive: true })
}

export function compileTypeScript(entryPath, distDir, externals = []) {
  rmSync(distDir, { recursive: true, force: true })
  const args = ["build", entryPath, "--outdir", distDir, "--target", "node", "--format", "esm"]
  for (const external of externals) {
    args.push("--external", external)
  }
  run("bun", args)
  return distDir
}

export function copyDistToPackage(distDir, packageRoot) {
  const destDir = join(packageRoot, "dist")
  rmSync(destDir, { recursive: true, force: true })
  cpSync(distDir, destDir, { recursive: true })
}

// The bundled Zellij binary is content-pinned by SHA-256, so a single verified
// copy can be cached and reused across builds (offline builds, faster CI).
// Callers MUST await this before reporting a successful build: on return the
// pinned Zellij binary is installed in packageRoot/bin/zellij and verified.
export async function ensureZellij(packageRoot) {
  const zellijBinDir = join(packageRoot, "bin", "zellij")
  const zellijBinPath = join(zellijBinDir, "zellij")

  if (existsSync(zellijBinPath)) {
    // Any validation failure (including EACCES on an unreadable file) must
    // fall through to a staged reinstall, never abort the build.
    try {
      const binStat = statSync(zellijBinPath)
      if (binStat.isFile() && (binStat.mode & 0o111) !== 0) {
        const existingHash = await sha256File(zellijBinPath)
        if (existingHash === ZELLIJ_BINARY_SHA256) {
          if ((binStat.mode & 0o777) !== 0o755) {
            chmodSync(zellijBinPath, 0o755)
          }
          console.log("Using existing bundled Zellij binary.")
          return
        }
        console.log(`Existing bundled Zellij hash mismatch (${existingHash}); reinstalling.`)
      } else {
        console.log("Existing bundled Zellij is not an executable file; reinstalling.")
      }
    } catch (err) {
      console.log(`Existing bundled Zellij validation failed (${err}); reinstalling.`)
    }
  }

  mkdirSync(zellijBinDir, { recursive: true })

  const zellijCacheRoot =
    process.env.AGENT_TERMINAL_ZELLIJ_CACHE ?? join(homedir(), ".cache", "agent-terminal-zellij")
  const zellijCachePath = join(zellijCacheRoot, `zellij-${ZELLIJ_VERSION}`)

  // Stage the binary at a temporary sibling of the final path and verify it
  // there, so a half-written or unverified binary can never be published:
  // only a fully verified file is atomically renamed onto zellijBinPath.
  const stagingPath = join(zellijBinDir, `zellij.tmp-${process.pid}`)
  const extractDir = join(zellijBinDir, `zellij.tmp-${process.pid}-extract`)

  try {
    // Prefer a previously verified copy from the local cache; only hit the
    // network (GitHub) when the cache misses.
    if (existsSync(zellijCachePath)) {
      try {
        cpSync(zellijCachePath, stagingPath)
        const cachedHash = await sha256File(stagingPath)
        if (cachedHash !== ZELLIJ_BINARY_SHA256) {
          console.log(`Cached Zellij hash mismatch (${cachedHash}); re-downloading.`)
        } else {
          chmodSync(stagingPath, 0o755)
          renameSync(stagingPath, zellijBinPath)
          console.log(`Using cached Zellij ${ZELLIJ_VERSION} (${zellijCachePath})`)
          return
        }
      } catch (error) {
        console.log(`Cached Zellij unusable (${error.message}); re-downloading.`)
      }
    }

    const archivePath = join(zellijBinDir, `zellij-${ZELLIJ_VERSION}.tar.gz`)
    console.log(`Downloading ${releaseUrl}`)

    const response = await fetch(releaseUrl)
    if (!response.ok) {
      throw new Error(`Failed to download Zellij: ${response.status} ${response.statusText}`)
    }

    if (response.body === null) {
      throw new Error("Download response body was empty")
    }

    const body = Readable.fromWeb(response.body)
    const fileStream = createWriteStream(archivePath)
    await finished(body.pipe(fileStream))

    const archiveHash = await sha256File(archivePath)
    if (archiveHash !== ZELLIJ_ARCHIVE_SHA256) {
      throw new Error(
        `Zellij archive hash mismatch.\nExpected: ${ZELLIJ_ARCHIVE_SHA256}\nActual:   ${archiveHash}`,
      )
    }

    mkdirSync(extractDir, { recursive: true })
    run("tar", ["-xzf", archivePath, "-C", extractDir])

    const stagedBinPath = join(extractDir, "zellij")
    const binaryHash = await sha256File(stagedBinPath)
    if (binaryHash !== ZELLIJ_BINARY_SHA256) {
      throw new Error(
        `Zellij binary hash mismatch.\nExpected: ${ZELLIJ_BINARY_SHA256}\nActual:   ${binaryHash}`,
      )
    }

    const versionResult = run(stagedBinPath, ["--version"], {
      stdio: ["ignore", "pipe", "inherit"],
    })
    const version = versionResult.stdout.toString().trim()
    if (!version.includes(ZELLIJ_VERSION)) {
      throw new Error(`Unexpected Zellij version: ${version}`)
    }

    chmodSync(stagedBinPath, 0o755)
    renameSync(stagedBinPath, zellijBinPath)
    rmSync(archivePath, { force: true })

    mkdirSync(zellijCacheRoot, { recursive: true })
    cpSync(zellijBinPath, zellijCachePath)
    console.log(`Bundled ${version} at ${zellijBinPath} (cached for future builds)`)
  } finally {
    rmSync(stagingPath, { force: true })
    rmSync(extractDir, { recursive: true, force: true })
  }
}

export function cleanPackage(packageRoot) {
  rmSync(join(packageRoot, "dist"), { recursive: true, force: true })
  rmSync(join(packageRoot, "skills"), { recursive: true, force: true })
  rmSync(join(packageRoot, "bin"), { recursive: true, force: true })
}
