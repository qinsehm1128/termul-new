#!/usr/bin/env node

import { spawn } from 'node:child_process'
import process from 'node:process'
import { canaryEnvironment } from './canary-profile.mjs'

const port = process.env.TAURI_DEV_PORT?.trim() || '5182'
const args = [
  'tauri',
  'dev',
  '--config',
  'src-tauri/tauri.conf.dev.json',
  '--config',
  'src-tauri/tauri.conf.canary.json',
  '--config',
  JSON.stringify({
    identifier: 'com.se-manager.app.canary.dev',
    build: { devUrl: `http://localhost:${port}` }
  })
]

const child = spawn('bunx', args, {
  cwd: process.cwd(),
  env: { ...canaryEnvironment('0.0.0-canary.dev'), TAURI_DEV_PORT: port },
  stdio: 'inherit'
})

child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal)
  else process.exit(code ?? 1)
})
