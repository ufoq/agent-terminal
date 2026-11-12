#!/usr/bin/env node
// Single-source release version management for agent-terminal.
//
// Canonical state lives in the repository-root `release.json`, which contains
// exactly one field: `version`. This CLI keeps that file, the published npm
// package manifests, and Cargo's package version in lockstep, and refuses to
// touch anything else.
//
// Commands:
//   check          Validate release.json, every allowlisted manifest, and
//                  Cargo.toml's [package] version. Read-only: never writes,
//                  exits nonzero on any mismatch.
//   sync <version> Preflight every source input, then write release.json, the
//                  allowlisted manifests, and Cargo.toml to <version> and
//                  rerun validation. Aborts without writing if preflight fails.
//
// All paths are derived from this file's location (import.meta.url), never the
// process working directory, and no Git operations are performed.

import { readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)))

const RELEASE_FILE = join(repoRoot, "release.json")
const CARGO_FILE = join(repoRoot, "Cargo.toml")
const CARGO_NAME = "agent-terminal"

// The complete allowlist of versioned npm manifests. `name` is the expected
// package name; anything outside this list is out of scope by design.
const MANIFESTS = [
  {
    relPath: "pi/npm/packages/pi-agent-terminal/package.json",
    name: "@ufoq/pi-agent-terminal",
  },
  {
    relPath: "pi/npm/packages/pi-agent-terminal-bundle-zellij/package.json",
    name: "@ufoq/pi-agent-terminal-bundle-zellij",
  },
]

// Strict semver.org grammar: MAJOR.MINOR.PATCH with optional prerelease and
// build metadata. `v` prefixes, leading zeros, and partial versions are invalid.
const SEMVER_RE =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?(?![\s\S])/

const USAGE = `Usage:
  node scripts/release-version.mjs check
  node scripts/release-version.mjs sync <version>`

function readJson(path) {
  const raw = readFileSync(path, "utf8")
  const parsed = JSON.parse(raw)
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${path}: expected a JSON object, got ${Array.isArray(parsed) ? "an array" : typeof parsed}`)
  }
  return parsed
}

function jsonText(obj) {
  return `${JSON.stringify(obj, null, 2)}\n`
}

// Reads only Cargo's root [package] section. The editor is deliberately
// narrow: it rejects missing, duplicate, or unexpected package structure rather
// than acting as a general TOML rewriter.
const stripHorizontal = (line) => line.replace(/^[ \t]+|[ \t]+$/g, "")

function parseCargo(raw) {
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f\u0085\u2028\u2029]/.test(raw)) {
    throw new Error("unsupported control characters")
  }
  if (raw.includes('\"\"\"') || raw.includes("'''")) {
    throw new Error("multiline strings are not supported")
  }
  if (raw.split("\n").some((line) => /^[ \t]*\[\[/.test(line))) {
    throw new Error("array tables are not supported")
  }
  if (
    raw
      .split("\n")
      .some(
        (line) =>
          /^[ \t]*\[.*\bpackage\b/.test(line) &&
          stripHorizontal(line) !== "[package]",
      )
  ) {
    throw new Error("noncanonical package headers are not supported")
  }
  const allowedHeaders = new Set([
    "[package]",
    "[dependencies]",
    "[dev-dependencies]",
    "[lints.rust]",
    "[lints.clippy]",
    "[profile.release]",
  ])
  const seenHeaders = new Set()
  for (const line of raw.split("\n")) {
    if (line.trim().startsWith("[") && !stripHorizontal(line).startsWith("[")) {
      throw new Error("unsupported whitespace before a table header")
    }
    if (!/^[ \t]*\[/.test(line)) continue
    const header = stripHorizontal(line)
    if (!allowedHeaders.has(header)) {
      throw new Error(`unsupported Cargo table header: ${header}`)
    }
    if (seenHeaders.has(header)) {
      throw new Error(`duplicate Cargo table header: ${header}`)
    }
    seenHeaders.add(header)
  }
  if (
    raw
      .split("\n")
      .some((line) =>
        /^[ \t]*(?:package[ \t]*[.=]|["'][^"']*["'][ \t]*[.=])/.test(line),
      )
  ) {
    throw new Error("package assignments and quoted top-level keys are not supported")
  }
  const quotedHeaders = raw
    .split("\n")
    .filter((line) => /^[ \t]*\[.*["'\\].*\]/.test(line))
  if (quotedHeaders.length > 0) {
    throw new Error("quoted or escaped table headers are not supported")
  }
  const headers = [...raw.matchAll(/^[ \t]*\[[ \t]*package[ \t]*\][^\n]*$/gm)]
  if (headers.length !== 1 || headers[0][0] !== "[package]") {
    throw new Error(
      `expected exactly one canonical [package] header, found ${headers.length}`,
    )
  }
  const header = headers[0]
  const start = header.index + header[0].length
  const remainder = raw.slice(start)
  const nextHeader = remainder.search(/\n[ \t]*\[/)
  const end = nextHeader === -1 ? raw.length : start + nextHeader
  const section = raw.slice(start, end)

  const quotedKeys = section
    .split("\n")
    .filter((line) => /^[ \t]*["']/.test(line))
  if (quotedKeys.length > 0) {
    throw new Error("quoted package keys are not supported")
  }

  const nameLines = section
    .split("\n")
    .filter((line) => /^[ \t]*name\b/.test(line))
  if (
    nameLines.length !== 1 ||
    !/^name[ \t]*=[ \t]*"([^"]*)"[ \t]*$/.test(nameLines[0])
  ) {
    throw new Error("expected exactly one canonical package name assignment")
  }
  const name = /^name[ \t]*=[ \t]*"([^"]*)"[ \t]*$/.exec(nameLines[0])[1]
  if (name !== CARGO_NAME) {
    throw new Error(`expected package name ${JSON.stringify(CARGO_NAME)}, got ${JSON.stringify(name)}`)
  }

  const versionLines = section
    .split("\n")
    .filter((line) => /^[ \t]*version\b/.test(line))
  if (
    versionLines.length !== 1 ||
    !/^version[ \t]*=[ \t]*"([^"]*)"[ \t]*$/.test(versionLines[0])
  ) {
    throw new Error("expected exactly one canonical package version assignment")
  }
  const version = /^version[ \t]*=[ \t]*"([^"]*)"[ \t]*$/.exec(versionLines[0])[1]

  return { raw, start, end, section, version }
}

function cargoWithVersion(cargo, version) {
  const replacement = `version = "${version}"`
  const section = cargo.section.replace(/^version[ \t]*=[ \t]*"[^"]*"[ \t]*$/m, replacement)
  return cargo.raw.slice(0, cargo.start) + section + cargo.raw.slice(cargo.end)
}

function isValidSemver(value) {
  return typeof value === "string" && SEMVER_RE.test(value)
}

// Reads every source input — release.json and each allowlisted manifest —
// and verifies each is a JSON object with the expected manifest name. The
// current version values are deliberately NOT validated here: sync exists to
// repair drift, so a mismatched or even invalid version must not block it.
// Returns an array of diagnostic strings; empty means preflight passed.
function preflight() {
  const problems = []

  let release
  try {
    release = readJson(RELEASE_FILE)
  } catch (error) {
    problems.push(`${RELEASE_FILE}: ${error.message}`)
  }

  let cargo
  try {
    cargo = parseCargo(readFileSync(CARGO_FILE, "utf8"))
  } catch (error) {
    problems.push(`${CARGO_FILE}: ${error.message}`)
  }

  const manifests = []
  for (const { relPath, name } of MANIFESTS) {
    const absPath = join(repoRoot, relPath)
    let manifest
    try {
      manifest = readJson(absPath)
    } catch (error) {
      problems.push(`${relPath}: ${error.message}`)
      continue
    }
    if (manifest.name !== name) {
      problems.push(`${relPath}: expected name ${JSON.stringify(name)}, got ${JSON.stringify(manifest.name)}`)
    }
    manifests.push(manifest)
  }

  if (problems.length > 0) {
    return { problems }
  }
  return { release, manifests, cargo, problems }
}

// Returns an array of diagnostic strings; empty means everything is valid.
function validate() {
  const problems = []

  let releaseVersion
  try {
    const release = readJson(RELEASE_FILE)
    if (typeof release.version !== "string") {
      problems.push(`${RELEASE_FILE}: "version" is missing or not a string`)
    } else if (!isValidSemver(release.version)) {
      problems.push(`${RELEASE_FILE}: "version" ${JSON.stringify(release.version)} is not a valid semver`)
    } else {
      releaseVersion = release.version
    }
  } catch (error) {
    problems.push(`${RELEASE_FILE}: ${error.message}`)
  }

  let cargoVersion
  try {
    cargoVersion = parseCargo(readFileSync(CARGO_FILE, "utf8")).version
  } catch (error) {
    problems.push(`${CARGO_FILE}: ${error.message}`)
  }

  for (const { relPath, name } of MANIFESTS) {
    const absPath = join(repoRoot, relPath)
    let manifest
    try {
      manifest = readJson(absPath)
    } catch (error) {
      problems.push(`${relPath}: ${error.message}`)
      continue
    }
    if (manifest.name !== name) {
      problems.push(`${relPath}: expected name ${JSON.stringify(name)}, got ${JSON.stringify(manifest.name)}`)
    }
    if (releaseVersion === undefined) {
      // release.json is broken; the manifest version cannot be checked against
      // it. The version-specific diagnostics above already cover the cause.
      continue
    }
    if (typeof manifest.version !== "string" || manifest.version !== releaseVersion) {
      problems.push(
        `${relPath}: expected version ${JSON.stringify(releaseVersion)} (from release.json), got ${JSON.stringify(manifest.version)}`,
      )
    }
  }

  if (releaseVersion !== undefined && cargoVersion !== undefined && cargoVersion !== releaseVersion) {
    problems.push(
      `Cargo.toml: expected package version ${JSON.stringify(releaseVersion)} (from release.json), got ${JSON.stringify(cargoVersion)}`,
    )
  }

  return problems
}

function fail(message, exitCode = 1) {
  process.stderr.write(`${message}\n`)
  process.exitCode = exitCode
}

function runCheck() {
  const problems = validate()
  if (problems.length > 0) {
    for (const problem of problems) {
      process.stderr.write(`ERROR: ${problem}\n`)
    }
    fail(`check failed: ${problems.length} problem${problems.length === 1 ? "" : "s"} found`, 1)
    return
  }
  const releaseVersion = readJson(RELEASE_FILE).version
  console.log(`check passed: all ${MANIFESTS.length} manifests and Cargo.toml match release.json version ${releaseVersion}`)
}

function runSync(version) {
  if (!isValidSemver(version)) {
    fail(`sync: ${JSON.stringify(version)} is not a valid semver version`, 2)
    return
  }

  const { release, manifests, cargo, problems } = preflight()
  if (problems.length > 0) {
    for (const problem of problems) {
      process.stderr.write(`ERROR: ${problem}\n`)
    }
    fail(`sync aborted: no files were written`, 1)
    return
  }

  let writeQueue
  let cargoContents
  try {
    writeQueue = [
      [RELEASE_FILE, jsonText({ ...release, version })],
      ...MANIFESTS.map(({ relPath }, index) => [
        join(repoRoot, relPath),
        jsonText({ ...manifests[index], version }),
      ]),
    ]
    cargoContents = cargoWithVersion(cargo, version)
  } catch (error) {
    fail(`sync aborted: ${error.message}; no files were written`, 1)
    return
  }

  try {
    for (const [path, contents] of writeQueue) {
      writeFileSync(path, contents)
    }
    writeFileSync(CARGO_FILE, cargoContents)
  } catch (error) {
    fail(`sync failed: ${error.message}`, 1)
    return
  }

  const problemsAfter = validate()
  if (problemsAfter.length > 0) {
    for (const problem of problemsAfter) {
      process.stderr.write(`ERROR: ${problem}\n`)
    }
    fail(`sync failed: ${problemsAfter.length} problem${problemsAfter.length === 1 ? "" : "s"} found after writing`, 1)
    return
  }

  console.log(`synced release.json, ${MANIFESTS.length} manifests, and Cargo.toml to version ${version}`)
}

const args = process.argv.slice(2)
if (args.length === 1 && args[0] === "check") {
  runCheck()
} else if (args.length === 2 && args[0] === "sync") {
  runSync(args[1])
} else {
  fail(USAGE, 2)
}
