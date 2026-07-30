#!/usr/bin/env node
// Build script for the @ufoq/opencode-agent-terminal npm package.
//
// This script is invoked by `bun run build` and `npm run prepublishOnly`.
// It does four things:
//   1. Copy the bundled skill into the package directory.
//   2. Build a static x86_64 musl agent-terminal binary.
//   3. Download and verify a pinned Zellij no-web musl binary.
//   4. Compile the TypeScript plugin entrypoint.

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
import { dirname, join } from "node:path"
import { Readable } from "node:stream"
import { finished } from "node:stream/promises"
import { fileURLToPath } from "node:url"

const ZELLIJ_VERSION = "0.44.3"
const ZELLIJ_ARCHIVE_SHA256 = "f901129919b0a405ac5f278f53acd7fde5d62401324c509b6233038d5c0ad1f9"
const ZELLIJ_BINARY_SHA256 = "a675b0106263113b9cb8f028649bad05c5d2283331fa62b2b36dd275aeaaa4d3"

const __dirname = dirname(fileURLToPath(import.meta.url))
const packageRoot = dirname(__dirname)
const repoRoot = dirname(dirname(dirname(packageRoot)))

const releaseUrl = `https://github.com/zellij-org/zellij/releases/download/v${ZELLIJ_VERSION}/zellij-no-web-x86_64-unknown-linux-musl.tar.gz`
const zellijBinDir = join(packageRoot, "bin", "zellij")
const zellijBinPath = join(zellijBinDir, "zellij")

function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    stdio: "inherit",
    cwd: packageRoot,
    ...options,
  })
  if (result.status !== 0) {
    throw new Error(`Command failed with exit ${result.status ?? "null"}: ${cmd} ${args.join(" ")}`)
  }
  return result
}

async function sha256File(path) {
  const hash = createHash("sha256")
  hash.update(await readFile(path))
  return hash.digest("hex")
}

async function downloadZellij() {
  if (existsSync(zellijBinPath) && (statSync(zellijBinPath).mode & 0o111) !== 0) {
    console.log("Using existing bundled Zellij binary.")
    return
  }

  mkdirSync(zellijBinDir, { recursive: true })

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
  console.log(`Bundled ${version} at ${zellijBinPath}`)
}

function buildAgentTerminal() {
  const targetBin = join(
    repoRoot,
    "target",
    "x86_64-unknown-linux-musl",
    "release",
    "agent-terminal",
  )
  const destBin = join(packageRoot, "bin", "linux-x64", "agent-terminal")

  mkdirSync(join(packageRoot, "bin", "linux-x64"), { recursive: true })

  const cargoPath = process.env["CARGO_HOME"]
    ? join(process.env["CARGO_HOME"], "bin", "cargo")
    : "cargo"

  run(cargoPath, ["build", "--release", "--target", "x86_64-unknown-linux-musl"], {
    cwd: repoRoot,
    env: { ...process.env, PATH: `/home/agent/.cargo/bin:${process.env["PATH"] ?? ""}` },
  })

  cpSync(targetBin, destBin, { force: true })
  chmodSync(destBin, 0o755)

  console.log(`Built agent-terminal at ${destBin}`)
}

function copySkill() {
  const source = join(repoRoot, "opencode", "skills")
  const dest = join(packageRoot, "skills")
  rmSync(dest, { recursive: true, force: true })
  cpSync(source, dest, { recursive: true })
  console.log(`Copied skills from ${source}`)
}

function cleanBuildOutput() {
  rmSync(join(packageRoot, "dist"), { recursive: true, force: true })
  rmSync(join(packageRoot, "skills"), { recursive: true, force: true })
  rmSync(join(packageRoot, "bin"), { recursive: true, force: true })
}

function compileTypeScript() {
  run("bun", ["build", "src/index.ts", "--outdir", "dist", "--target", "node", "--format", "esm"])
  run("tsc", ["--emitDeclarationOnly"])
  console.log("Compiled TypeScript plugin entrypoint.")
}

async function main() {
  cleanBuildOutput()
  copySkill()
  buildAgentTerminal()
  await downloadZellij()
  compileTypeScript()
  console.log("Package build complete.")
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
