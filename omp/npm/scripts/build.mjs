#!/usr/bin/env node
// Build script for the @ufoq/omp-agent-terminal npm packages.
//
// This script builds two sibling packages from one shared source:
//   1. packages/omp-agent-terminal/ — agent-terminal + skill only
//   2. packages/omp-agent-terminal-bundle-zellij/ — agent-terminal + skill + pinned Zellij
//
// It runs from the repository root (`../../` relative to this script) so that
// the Rust binary is built once and then copied into both package outputs.
//
// The heavy lifting (Rust build, skill copy, TS compile, Zellij install) lives
// in the shared build library at scripts/npm-build-lib.mjs; the Zellij version
// and SHA-256 pins exist only there.

import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import {
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
const packagesDir = join(npmRoot, "packages")
const sharedSrc = join(npmRoot, "src", "index.ts")

const slimPackage = join(packagesDir, "omp-agent-terminal")
const bundlePackage = join(packagesDir, "omp-agent-terminal-bundle-zellij")

async function main() {
  cleanPackage(slimPackage)
  cleanPackage(bundlePackage)

  const targetBin = buildAgentTerminalBinary()
  const distDir = compileTypeScript(sharedSrc, join(npmRoot, "dist"), [])
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
