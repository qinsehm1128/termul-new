#!/usr/bin/env node

import { spawnSync } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import process from 'node:process'
import { canaryEnvironment, resolveCanaryVersion } from './canary-profile.mjs'

const packageJson = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'))
const version = resolveCanaryVersion(packageJson.version)
const noSign = process.env.CANARY_NO_SIGN === '1'
const publicKey = process.env.TAURI_CANARY_SIGNING_PUBLIC_KEY?.trim()
if (!noSign && !publicKey) {
  throw new Error(
    'TAURI_CANARY_SIGNING_PUBLIC_KEY is required for signed Canary builds (use CANARY_NO_SIGN=1 only for local unsigned verification)'
  )
}
const configOverride = JSON.stringify({
  version,
  ...(publicKey ? { plugins: { updater: { pubkey: publicKey } } } : {})
})
const args = [
  'tauri',
  'build',
  '--ci',
  ...(noSign ? ['--no-sign'] : []),
  '--config',
  'src-tauri/tauri.conf.prod.json',
  '--config',
  'src-tauri/tauri.conf.canary.json',
  '--config',
  configOverride
]

const result = spawnSync('bunx', args, {
  cwd: process.cwd(),
  env: canaryEnvironment(version),
  stdio: 'inherit'
})

if (result.error) throw result.error
process.exit(result.status ?? 1)
