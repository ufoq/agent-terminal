#!/usr/bin/env node
// Build script for the @ufoq/pi-agent-terminal npm packages.
//
// This script builds two sibling packages from one shared source:
//   1. packages/pi-agent-terminal/ — agent-terminal + skill only
//   2. packages/pi-agent-terminal-bundle-zellij/ — agent-terminal + skill + pinned Zellij
//
// It runs from the repository root (`../../` relative to this script) so that
// the Rust binary is built once and then copied into both package outputs.
//
// The heavy lifting (Rust build, skill copy, TS compile, Zellij install) lives
// in the shared build library at scripts/npm-build-lib.mjs; the Zellij version
// and SHA-256 pins exist only there.

import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import {
  ZELLIJ_VERSION,
  buildAgentTerminalBinary,
  cleanPackage,
  compileTypeScript,
  copyBinaryToPackage,
  copyDistToPackage,
  copySkillToPackage,
  ensureZellij,
  run,
} from "../../../scripts/npm-build-lib.mjs"

const __dirname = dirname(fileURLToPath(import.meta.url))
const npmRoot = dirname(__dirname)
const repoRoot = dirname(dirname(npmRoot))
const packagesDir = join(npmRoot, "packages")
const sharedSrc = join(npmRoot, "src", "index.ts")

const slimPackage = join(packagesDir, "pi-agent-terminal")
const bundlePackage = join(packagesDir, "pi-agent-terminal-bundle-zellij")

async function main() {
  run("node", [join(repoRoot, "scripts", "release-version.mjs"), "check"])
  const bundleManifest = JSON.parse(readFileSync(join(bundlePackage, "package.json"), "utf8"))
  if (bundleManifest.agentTerminal?.zellijVersion !== ZELLIJ_VERSION) {
    throw new Error(
      `bundle manifest declares Zellij ${JSON.stringify(bundleManifest.agentTerminal?.zellijVersion)}, expected ${ZELLIJ_VERSION}`,
    )
  }
  cleanPackage(slimPackage)
  cleanPackage(bundlePackage)

  const targetBin = buildAgentTerminalBinary()
  const releaseVersion = JSON.parse(readFileSync(join(repoRoot, "release.json"), "utf8")).version
  const builtVersion = run(targetBin, ["--version"], { stdio: "pipe" }).stdout.toString().trim()
  if (builtVersion !== `agent-terminal ${releaseVersion}`) {
    throw new Error(
      `built binary reports ${JSON.stringify(builtVersion)}, expected agent-terminal ${releaseVersion}`,
    )
  }
  const distDir = compileTypeScript(sharedSrc, join(npmRoot, "dist"), [
    "@earendil-works/pi-coding-agent",
  ])
  run("tsc", ["--emitDeclarationOnly"], { cwd: npmRoot })

  for (const packageRoot of [slimPackage, bundlePackage]) {
    copyBinaryToPackage(targetBin, packageRoot)
    copySkillToPackage(packageRoot)
    copyDistToPackage(distDir, packageRoot)
  }

  await ensureZellij(bundlePackage)

  console.log("Both packages built.")
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
