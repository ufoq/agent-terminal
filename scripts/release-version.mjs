#!/usr/bin/env node
// Single-source npm release version management for agent-terminal.
//
// Canonical state lives in the repository-root `release.json`, which contains
// exactly one field: `version`. This CLI keeps that file and the six published
// npm package manifests in lockstep, and refuses to touch anything else.
//
// Commands:
//   check          Validate release.json and every allowlisted manifest.
//                  Read-only: never writes, exits nonzero on any mismatch.
//   sync <version> Preflight every source input (readable JSON object with
//                  the expected manifest name), then write release.json and
//                  the six allowlisted manifests to <version> and run the
//                  same validation. Aborts without writing if preflight fails.
//
// All paths are derived from this file's location (import.meta.url), never the
// process working directory, and no Git operations are performed.

import { readFileSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const repoRoot = dirname(dirname(fileURLToPath(import.meta.url)))

const RELEASE_FILE = join(repoRoot, "release.json")

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
  {
    relPath: "omp/npm/packages/omp-agent-terminal/package.json",
    name: "@ufoq/omp-agent-terminal",
  },
  {
    relPath: "omp/npm/packages/omp-agent-terminal-bundle-zellij/package.json",
    name: "@ufoq/omp-agent-terminal-bundle-zellij",
  },
  {
    relPath: "opencode/npm/packages/opencode-agent-terminal/package.json",
    name: "@ufoq/opencode-agent-terminal",
  },
  {
    relPath: "opencode/npm/packages/opencode-agent-terminal-bundle-zellij/package.json",
    name: "@ufoq/opencode-agent-terminal-bundle-zellij",
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

function writeJson(path, obj) {
  writeFileSync(path, `${JSON.stringify(obj, null, 2)}\n`)
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
  return { release, manifests, problems }
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
  console.log(`check passed: all ${MANIFESTS.length} manifests match release.json version ${releaseVersion}`)
}

function runSync(version) {
  if (!isValidSemver(version)) {
    fail(`sync: ${JSON.stringify(version)} is not a valid semver version`, 2)
    return
  }

  const { release, manifests, problems } = preflight()
  if (problems.length > 0) {
    for (const problem of problems) {
      process.stderr.write(`ERROR: ${problem}\n`)
    }
    fail(`sync aborted: no files were written`, 1)
    return
  }

  const writeQueue = [
    [RELEASE_FILE, { ...release, version }],
    ...MANIFESTS.map(({ relPath }, index) => [
      join(repoRoot, relPath),
      { ...manifests[index], version },
    ]),
  ]

  try {
    for (const [path, obj] of writeQueue) {
      writeJson(path, obj)
    }
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

  console.log(`synced release.json and ${MANIFESTS.length} manifests to version ${version}`)
}

const args = process.argv.slice(2)
if (args.length === 1 && args[0] === "check") {
  runCheck()
} else if (args.length === 2 && args[0] === "sync") {
  runSync(args[1])
} else {
  fail(USAGE, 2)
}
