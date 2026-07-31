#!/usr/bin/env node
// Build script for the @ufoq/opencode-agent-terminal npm packages.
//
// This script builds two sibling packages from one shared source:
//   1. packages/opencode-agent-terminal/ — agent-terminal + skill only
//   2. packages/opencode-agent-terminal-bundle-zellij/ — agent-terminal + skill + pinned Zellij
//
// It runs from the repository root (`../../` relative to this script) so that
// the Rust binary is built once and then copied into both package outputs.

import { createHash } from "node:crypto"
import { spawnSync } from "node:child_process"
import {
  chmodSync,
  cpSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  rmSync,
  statSync,
} from "node:fs"
import { readFile } from "node:fs/promises"
import { homedir } from "node:os"
import { dirname, join } from "node:path"
import { Readable } from "node:stream"
import { finished } from "node:stream/promises"
import { fileURLToPath } from "node:url"

const ZELLIJ_VERSION = "0.44.3"
const ZELLIJ_ARCHIVE_SHA256 = "f901129919b0a405ac5f278f53acd7fde5d62401324c509b6233038d5c0ad1f9"
const ZELLIJ_BINARY_SHA256 = "a675b0106263113b9cb8f028649bad05c5d2283331fa62b2b36dd275aeaaa4d3"

const __dirname = dirname(fileURLToPath(import.meta.url))
const npmRoot = dirname(__dirname)
const repoRoot = dirname(dirname(npmRoot))
const packagesDir = join(npmRoot, "packages")
const sharedSrc = join(npmRoot, "src", "index.ts")

const slimPackage = join(packagesDir, "opencode-agent-terminal")
const bundlePackage = join(packagesDir, "opencode-agent-terminal-bundle-zellij")

const releaseUrl = `https://github.com/zellij-org/zellij/releases/download/v${ZELLIJ_VERSION}/zellij-no-web-x86_64-unknown-linux-musl.tar.gz`

function run(cmd, args, options = {}) {
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

async function sha256File(path) {
  const data = await readFile(path)
  return createHash("sha256").update(data).digest("hex")
}

function buildAgentTerminal() {
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

function copyBinaryToPackage(targetBin, packageRoot) {
  const destDir = join(packageRoot, "bin", "linux-x64")
  const destBin = join(destDir, "agent-terminal")
  mkdirSync(destDir, { recursive: true })
  cpSync(targetBin, destBin, { force: true })
  chmodSync(destBin, 0o755)
}

function copySkillToPackage(packageRoot) {
  const source = join(repoRoot, "opencode", "skills")
  const dest = join(packageRoot, "skills")
  rmSync(dest, { recursive: true, force: true })
  cpSync(source, dest, { recursive: true })
}

function compileTypeScript() {
  const distDir = join(npmRoot, "dist")
  rmSync(distDir, { recursive: true, force: true })
  run("bun", ["build", sharedSrc, "--outdir", distDir, "--target", "node", "--format", "esm"], {
    cwd: npmRoot,
  })
  run("tsc", ["--emitDeclarationOnly"], { cwd: npmRoot })
  return distDir
}

function copyDistToPackage(distDir, packageRoot) {
  const destDir = join(packageRoot, "dist")
  rmSync(destDir, { recursive: true, force: true })
  cpSync(distDir, destDir, { recursive: true })
}

// The bundled Zellij binary is content-pinned by SHA-256, so a single verified
// copy can be cached and reused across builds (offline builds, faster CI).
const zellijCacheRoot =
  process.env.AGENT_TERMINAL_ZELLIJ_CACHE ?? join(homedir(), ".cache", "agent-terminal-zellij")
const zellijCachePath = join(zellijCacheRoot, `zellij-${ZELLIJ_VERSION}`)

async function downloadZellij(packageRoot) {
  const zellijBinDir = join(packageRoot, "bin", "zellij")
  const zellijBinPath = join(zellijBinDir, "zellij")

  if (existsSync(zellijBinPath) && (statSync(zellijBinPath).mode & 0o111) !== 0) {
    console.log("Using existing bundled Zellij binary.")
    return
  }

  mkdirSync(zellijBinDir, { recursive: true })

  // Prefer a previously verified copy from the local cache; only hit the
  // network (GitHub) when the cache misses.
  if (existsSync(zellijCachePath)) {
    const cachedHash = await sha256File(zellijCachePath)
    if (cachedHash === ZELLIJ_BINARY_SHA256) {
      cpSync(zellijCachePath, zellijBinPath)
      chmodSync(zellijBinPath, 0o755)
      console.log(`Using cached Zellij ${ZELLIJ_VERSION} (${zellijCachePath})`)
      return
    }
    console.log(`Cached Zellij hash mismatch (${cachedHash}); re-downloading.`)
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

  run("tar", ["-xzf", archivePath, "-C", zellijBinDir])

  const binaryHash = await sha256File(zellijBinPath)
  if (binaryHash !== ZELLIJ_BINARY_SHA256) {
    throw new Error(
      `Zellij binary hash mismatch.\nExpected: ${ZELLIJ_BINARY_SHA256}\nActual:   ${binaryHash}`,
    )
  }

  const versionResult = run(zellijBinPath, ["--version"], {
    stdio: ["ignore", "pipe", "inherit"],
  })
  const version = versionResult.stdout.toString().trim()
  if (!version.includes(ZELLIJ_VERSION)) {
    throw new Error(`Unexpected Zellij version: ${version}`)
  }

  chmodSync(zellijBinPath, 0o755)
  rmSync(archivePath, { force: true })

  mkdirSync(zellijCacheRoot, { recursive: true })
  cpSync(zellijBinPath, zellijCachePath)
  console.log(`Bundled ${version} at ${zellijBinPath} (cached for future builds)`)
}

function cleanPackage(packageRoot) {
  rmSync(join(packageRoot, "dist"), { recursive: true, force: true })
  rmSync(join(packageRoot, "skills"), { recursive: true, force: true })
  rmSync(join(packageRoot, "bin"), { recursive: true, force: true })
}

async function main() {
  cleanPackage(slimPackage)
  cleanPackage(bundlePackage)

  const targetBin = buildAgentTerminal()
  const distDir = compileTypeScript()

  for (const packageRoot of [slimPackage, bundlePackage]) {
    copyBinaryToPackage(targetBin, packageRoot)
    copySkillToPackage(packageRoot)
    copyDistToPackage(distDir, packageRoot)
  }

  await downloadZellij(bundlePackage)

  console.log("Both packages built.")
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
