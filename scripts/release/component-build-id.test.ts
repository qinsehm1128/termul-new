import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, test } from 'vitest'
// @ts-expect-error The production helper is intentionally plain ESM for direct workflow use.
import { explainIdentityChange, generateComponentIdentities } from './component-build-id.mjs'

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

describe('scoped Cargo material for Core identities', () => {
  const scopedInputs = {
    schemaVersion: 1,
    sharedFiles: [],
    cargo: {
      manifest: 'src-tauri/Cargo.toml',
      lock: 'src-tauri/Cargo.lock',
      package: 'se-manager',
      crateSource: 'src-tauri/src',
      workspaceCrates: 'src-tauri/crates',
      scopedComponents: ['acpCore', 'terminalCore']
    },
    components: {
      renderer: [],
      guiNative: ['src-tauri/Cargo.toml', 'src-tauri/Cargo.lock'],
      acpCore: ['src-tauri/src/acp.rs'],
      terminalCore: ['src-tauri/src/pty.rs', 'src-tauri/crates/se-pty']
    }
  }

  type Package = { name: string; version: string; deps?: string[] }

  function manifest({
    version = '0.14.10',
    tokio = '{ version = "1", features = ["rt"] }',
    extraDependency = '',
    comment = '# PTY runtime',
    profile = 'opt-level = 3'
  } = {}) {
    return [
      '[package]',
      'name = "se-manager"',
      `version = "${version}"`,
      '',
      '[profile.release]',
      profile,
      '',
      '[dependencies]',
      comment,
      'se-pty = { path = "crates/se-pty" }',
      `tokio = ${tokio}`,
      'tauri = { version = "2", features = [',
      '    "unstable",',
      '] }',
      extraDependency,
      '',
      '[dev-dependencies]',
      'tempfile = "3"',
      ''
    ].join('\n')
  }

  function lock(packages: Package[]) {
    return [
      'version = 4',
      ...packages.map((entry) =>
        [
          '',
          '[[package]]',
          `name = "${entry.name}"`,
          `version = "${entry.version}"`,
          ...(entry.deps?.length
            ? ['dependencies = [', ...entry.deps.map((dep) => ` "${dep}",`), ']']
            : [])
        ].join('\n')
      ),
      ''
    ].join('\n')
  }

  const basePackages = (extra: Package[] = [], mio = '1.0.0'): Package[] => [
    {
      name: 'se-manager',
      version: '0.14.10',
      deps: ['se-pty', 'tauri', 'tokio', ...extra.map((p) => p.name)]
    },
    { name: 'se-pty', version: '0.1.0', deps: ['tokio'] },
    { name: 'tokio', version: '1.40.0', deps: ['mio'] },
    { name: 'mio', version: mio },
    { name: 'tauri', version: '2.0.0' },
    ...extra
  ]

  async function scopedRoot() {
    const root = await mkdtemp(join(tmpdir(), 'se-component-cargo-'))
    await mkdir(join(root, 'src-tauri/src'), { recursive: true })
    await mkdir(join(root, 'src-tauri/crates/se-pty/src'), { recursive: true })
    await writeFile(join(root, 'src-tauri/Cargo.toml'), manifest())
    await writeFile(join(root, 'src-tauri/Cargo.lock'), lock(basePackages()))
    await writeFile(
      join(root, 'src-tauri/crates/se-pty/Cargo.toml'),
      '[package]\nname = "se-pty"\nversion = "0.1.0"\n'
    )
    await writeFile(join(root, 'src-tauri/crates/se-pty/src/lib.rs'), 'pub fn spawn() {}\n')
    await writeFile(join(root, 'src-tauri/src/pty.rs'), 'use tokio::sync::Mutex;\n')
    await writeFile(join(root, 'src-tauri/src/acp.rs'), 'pub const ACP: u8 = 1;\n')
    return root
  }

  async function ids(root: string) {
    return (await generateComponentIdentities({ root, inputs: scopedInputs })).components
  }

  test('a dependency only the GUI uses leaves the Core identity alone', async () => {
    const root = await scopedRoot()
    try {
      const before = await ids(root)
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        manifest({ extraDependency: 'objc2 = "0.6"' })
      )
      await writeFile(
        join(root, 'src-tauri/Cargo.lock'),
        lock(basePackages([{ name: 'objc2', version: '0.6.4' }]))
      )
      const after = await ids(root)

      expect(after.terminalCore.buildId).toBe(before.terminalCore.buildId)
      expect(after.guiNative.buildId).not.toBe(before.guiNative.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('comments, the package version and dev-dependencies leave it alone', async () => {
    const root = await scopedRoot()
    try {
      const before = await ids(root)
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        manifest({ version: '0.14.11', comment: '# reworded comment' }).replace(
          'tempfile = "3"',
          'tempfile = "4"'
        )
      )
      expect((await ids(root)).terminalCore.buildId).toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('a dev-dependency stays out even when Core test code names it', async () => {
    const root = await scopedRoot()
    try {
      await writeFile(
        join(root, 'src-tauri/src/pty.rs'),
        'use tokio::sync::Mutex;\n#[cfg(test)]\nmod tests { fn dir() { tempfile::tempdir(); } }\n'
      )
      await writeFile(
        join(root, 'src-tauri/Cargo.lock'),
        lock(basePackages([{ name: 'tempfile', version: '3.0.0' }]))
      )
      const before = await ids(root)
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        manifest().replace('tempfile = "3"', 'tempfile = "4"')
      )
      await writeFile(
        join(root, 'src-tauri/Cargo.lock'),
        lock(basePackages([{ name: 'tempfile', version: '4.0.0' }]))
      )
      expect((await ids(root)).terminalCore.buildId).toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('comments anywhere in the manifest leave it alone', async () => {
    const root = await scopedRoot()
    try {
      const before = await ids(root)
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        manifest({
          profile: 'opt-level = 3 # tuned for size later',
          tokio: '{ version = "1", features = ["rt"] } # runtime'
        }).replace('[package]', '# The desktop app\n[package]')
      )
      expect((await ids(root)).terminalCore.buildId).toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('a feature on a later line of a multi-line entry changes it', async () => {
    const root = await scopedRoot()
    const multiLine = (features: string) =>
      `{ version = "1", features = [\n    "rt",\n${features}] }`
    try {
      await writeFile(join(root, 'src-tauri/Cargo.toml'), manifest({ tokio: multiLine('') }))
      const before = await ids(root)
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        manifest({ tokio: multiLine('    "net",\n') })
      )
      expect((await ids(root)).terminalCore.buildId).not.toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('a dependency the Core names changes it', async () => {
    const root = await scopedRoot()
    try {
      const before = await ids(root)
      await writeFile(
        join(root, 'src-tauri/Cargo.toml'),
        manifest({ tokio: '{ version = "1", features = ["rt", "net"] }' })
      )
      expect((await ids(root)).terminalCore.buildId).not.toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('a transitive lock change under a Core dependency changes it', async () => {
    const root = await scopedRoot()
    try {
      const before = await ids(root)
      await writeFile(join(root, 'src-tauri/Cargo.lock'), lock(basePackages([], '1.0.1')))
      expect((await ids(root)).terminalCore.buildId).not.toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('a shared manifest section such as the release profile changes it', async () => {
    const root = await scopedRoot()
    try {
      const before = await ids(root)
      await writeFile(join(root, 'src-tauri/Cargo.toml'), manifest({ profile: 'opt-level = 2' }))
      expect((await ids(root)).terminalCore.buildId).not.toBe(before.terminalCore.buildId)
    } finally {
      await rm(root, { recursive: true })
    }
  })

  test('roots are the dependencies the Core names plus the workspace crates it lists', async () => {
    const root = await scopedRoot()
    try {
      const result = await generateComponentIdentities({ root, inputs: scopedInputs })
      expect(result.payloads.terminalCore.cargo.roots).toEqual(['se-pty', 'tokio'])
      // The ACP fixture names no dependency: nothing but the shared manifest part.
      expect(result.payloads.acpCore.cargo.roots).toEqual([])
    } finally {
      await rm(root, { recursive: true })
    }
  })
})

describe('explainIdentityChange', () => {
  const identity = (
    terminalFiles: Array<[string, string]>,
    cargo?: { roots: string[]; manifestSha256: string; lockSha256: string }
  ) => {
    const payload = (files: Array<[string, string]>) => ({
      files: files.map(([path, sha256]) => ({ path, sha256 }))
    })
    const id = JSON.stringify([terminalFiles, cargo])
    return {
      components: {
        renderer: { buildId: 'r' },
        guiNative: { buildId: 'g' },
        acpCore: { buildId: 'a' },
        terminalCore: { buildId: id }
      },
      payloads: {
        renderer: payload([]),
        guiNative: payload([]),
        acpCore: payload([]),
        terminalCore: { ...payload(terminalFiles), cargo }
      }
    }
  }

  test('names the files and Cargo parts behind a changed identity', () => {
    const before = identity(
      [
        ['core/terminal.rs', '1'],
        ['core/launcher.rs', '1']
      ],
      { roots: ['tokio', 'tauri-plugin-log'], manifestSha256: 'm', lockSha256: 'l' }
    )
    const after = identity(
      [
        ['core/terminal.rs', '2'],
        ['core/process.rs', '1']
      ],
      { roots: ['tokio', 'uuid'], manifestSha256: 'm', lockSha256: 'l2' }
    )
    expect(explainIdentityChange(before, after)).toEqual([
      'renderer: unchanged',
      'guiNative: unchanged',
      'acpCore: unchanged',
      'terminalCore: changed',
      '  ~ core/terminal.rs',
      '  + core/process.rs',
      '  - core/launcher.rs',
      '  + cargo dependency uuid',
      '  - cargo dependency tauri-plugin-log',
      '  ~ Cargo.lock (scoped part)'
    ])
  })
})
