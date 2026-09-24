#!/usr/bin/env node

import { createHash } from 'node:crypto'
import { readdir, readFile, writeFile } from 'node:fs/promises'
import { relative, resolve, sep } from 'node:path'
import process from 'node:process'
import { pathToFileURL } from 'node:url'

const DEFAULT_INPUTS = new URL('./component-build-inputs.json', import.meta.url)
const ID_SCHEMA_VERSION = 1
const COMPONENTS = ['renderer', 'guiNative', 'acpCore', 'terminalCore']

function assertPlainObject(value, description) {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${description} must be an object`)
  }
}

function normalizeText(path, content) {
  const normalized = content.replaceAll('\r\n', '\n').replaceAll('\r', '\n')
  if (path !== 'src-tauri/Cargo.toml' && path !== 'src-tauri/Cargo.lock') return normalized

  return normalized
    .replace(/^version = "[^"]+"$/m, 'version = "<package-version>"')
    .replace(/(name = "se-manager"\nversion = )"[^"]+"/g, '$1"<package-version>"')
}

function isInside(root, candidate) {
  const rootRelative = relative(root, candidate)
  return rootRelative === '' || (rootRelative !== '..' && !rootRelative.startsWith(`..${sep}`))
}

async function expandInput(root, input) {
  const absolute = resolve(root, input)
  if (!isInside(root, absolute)) {
    throw new Error(`Input path escapes repository root: ${input}`)
  }

  let stat
  try {
    stat = await readdir(absolute, { withFileTypes: true })
  } catch (error) {
    if (error?.code === 'ENOTDIR') return [input]
    throw new Error(`Missing component identity input: ${input}`)
  }

  const files = []
  for (const entry of stat.sort((left, right) => left.name.localeCompare(right.name))) {
    const child = `${input}/${entry.name}`
    if (entry.isDirectory()) files.push(...(await expandInput(root, child)))
    else if (entry.isFile()) files.push(child)
  }
  return files
}

async function readInputs(root, inputs) {
  assertPlainObject(inputs, 'component identity inputs')
  if (inputs.schemaVersion !== ID_SCHEMA_VERSION) {
    throw new Error(`component identity input schemaVersion must be ${ID_SCHEMA_VERSION}`)
  }
  if (
    !Array.isArray(inputs.sharedFiles) ||
    !inputs.sharedFiles.every((value) => typeof value === 'string')
  ) {
    throw new Error('component identity sharedFiles must be an array of strings')
  }
  assertPlainObject(inputs.components, 'component identity components')

  const expanded = new Map()
  const add = async (path) => {
    for (const file of await expandInput(root, path)) expanded.set(file, true)
  }

  for (const path of inputs.sharedFiles) await add(path)
  for (const component of COMPONENTS) {
    const paths = inputs.components[component]
    if (!Array.isArray(paths) || !paths.every((value) => typeof value === 'string')) {
      throw new Error(`component identity inputs missing ${component}`)
    }
    for (const path of paths) await add(path)
  }

  const fileDigests = new Map()
  for (const path of [...expanded.keys()].sort()) {
    const content = await readFile(resolve(root, path))
    const text = normalizeText(path, content.toString('utf8'))
    const digest = createHash('sha256').update(text, 'utf8').digest('hex')
    fileDigests.set(path, digest)
  }

  return { fileDigests }
}

function componentPayload(component, inputs, fileDigests) {
  const shared = [...inputs.sharedFiles].sort()
  const own = [...inputs.components[component]].sort()
  const paths = new Set([...shared, ...own])
  const files = [...fileDigests.entries()]
    .filter(
      ([path]) => paths.has(path) || [...paths].some((prefix) => path.startsWith(`${prefix}/`))
    )
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([path, digest]) => ({ path, sha256: digest }))

  return {
    schemaVersion: ID_SCHEMA_VERSION,
    component,
    files
  }
}

export async function generateComponentIdentities({ root = process.cwd(), inputs }) {
  const normalizedRoot = resolve(root)
  const source = inputs ?? JSON.parse(await readFile(DEFAULT_INPUTS, 'utf8'))
  const { fileDigests } = await readInputs(normalizedRoot, source)
  const components = {}
  const payloads = {}

  for (const component of COMPONENTS) {
    const payload = componentPayload(component, source, fileDigests)
    const digest = createHash('sha256').update(JSON.stringify(payload), 'utf8').digest('hex')
    components[component] = {
      buildId: `termul-${component}-v${ID_SCHEMA_VERSION}-sha256:${digest}`
    }
    payloads[component] = payload
  }

  return {
    schemaVersion: ID_SCHEMA_VERSION,
    components,
    inputs: {
      sharedFiles: [...source.sharedFiles].sort(),
      components: Object.fromEntries(
        COMPONENTS.map((component) => [component, [...source.components[component]].sort()])
      )
    },
    payloads
  }
}

async function runCli(argv) {
  let inputsPath = DEFAULT_INPUTS
  let outputPath = null
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--inputs') inputsPath = resolve(argv[++index])
    else if (argument === '--output') outputPath = resolve(argv[++index])
    else if (argument === '--help') {
      process.stdout.write('Usage: component-build-id.mjs [--inputs <path>] [--output <path>]\n')
      return
    } else throw new Error(`Unknown argument: ${argument}`)
  }

  const result = await generateComponentIdentities({
    root: process.cwd(),
    inputs: JSON.parse(await readFile(inputsPath, 'utf8'))
  })
  const serialized = `${JSON.stringify(result, null, 2)}\n`
  if (outputPath) await writeFile(outputPath, serialized)
  else process.stdout.write(serialized)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  runCli(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}
