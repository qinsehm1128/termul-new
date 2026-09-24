import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, test } from 'vitest'
// @ts-expect-error The production helper is intentionally plain ESM for direct workflow use.
import { generateComponentIdentities } from './component-build-id.mjs'

const inputs = {
  schemaVersion: 1,
  sharedFiles: ['src-tauri/Cargo.toml', 'src-tauri/src/core/ipc.rs'],
  components: {
    renderer: [],
    guiNative: [],
    acpCore: ['src-tauri/src/acp.rs'],
    terminalCore: ['src-tauri/src/pty.rs']
  }
}

async function fixtureRoot(version = '0.12.6') {
  const root = await mkdtemp(join(tmpdir(), 'se-component-build-id-'))
  await mkdir(join(root, 'src-tauri/src/core'), { recursive: true })
  await writeFile(
    join(root, 'src-tauri/Cargo.toml'),
    `[package]\nname = "se-manager"\nversion = "${version}"\n`
  )
  await writeFile(join(root, 'src-tauri/src/core/ipc.rs'), 'pub const PROTOCOL: u8 = 1;\n')
  await writeFile(join(root, 'src-tauri/src/acp.rs'), 'pub const ACP: u8 = 1;\n')
  await writeFile(join(root, 'src-tauri/src/pty.rs'), 'pub const PTY: u8 = 1;\n')
  return root
}

describe('generateComponentIdentities', () => {
  test('is deterministic and excludes package version from Core IDs', async () => {
    const firstRoot = await fixtureRoot('0.12.6')
    const secondRoot = await fixtureRoot('0.12.7')
    try {
      const first = await generateComponentIdentities({ root: firstRoot, inputs })
      const second = await generateComponentIdentities({ root: secondRoot, inputs })

      expect(first.components.acpCore.buildId).toBe(second.components.acpCore.buildId)
      expect(first.components.terminalCore.buildId).toBe(second.components.terminalCore.buildId)
      expect(JSON.stringify(first)).toBe(
        JSON.stringify(await generateComponentIdentities({ root: firstRoot, inputs }))
      )
    } finally {
      await Promise.all([rm(firstRoot, { recursive: true }), rm(secondRoot, { recursive: true })])
    }
  })

  test('changes only the ACP identity for an ACP-only source change', async () => {
    const root = await fixtureRoot()
    try {
      const before = await generateComponentIdentities({ root, inputs })
      await writeFile(join(root, 'src-tauri/src/acp.rs'), 'pub const ACP: u8 = 2;\n')
      const after = await generateComponentIdentities({ root, inputs })

      expect(after.components.acpCore.buildId).not.toBe(before.components.acpCore.buildId)
      expect(after.components.terminalCore.buildId).toBe(before.components.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('changes only the Terminal identity for a Terminal-only source change', async () => {
    const root = await fixtureRoot()
    try {
      const before = await generateComponentIdentities({ root, inputs })
      await writeFile(join(root, 'src-tauri/src/pty.rs'), 'pub const PTY: u8 = 2;\n')
      const after = await generateComponentIdentities({ root, inputs })

      expect(after.components.terminalCore.buildId).not.toBe(before.components.terminalCore.buildId)
      expect(after.components.acpCore.buildId).toBe(before.components.acpCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('changes both Core identities for a shared protocol change', async () => {
    const root = await fixtureRoot()
    try {
      const before = await generateComponentIdentities({ root, inputs })
      await writeFile(join(root, 'src-tauri/src/core/ipc.rs'), 'pub const PROTOCOL: u8 = 2;\n')
      const after = await generateComponentIdentities({ root, inputs })

      expect(after.components.acpCore.buildId).not.toBe(before.components.acpCore.buildId)
      expect(after.components.terminalCore.buildId).not.toBe(before.components.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('fails when an identity input is missing', async () => {
    const root = await fixtureRoot()
    try {
      await expect(
        generateComponentIdentities({
          root,
          inputs: { ...inputs, sharedFiles: [...inputs.sharedFiles, 'src-tauri/missing.rs'] }
        })
      ).rejects.toThrow('Missing component identity input')
    } finally {
      await rm(root, { recursive: true })
    }
  })
})
