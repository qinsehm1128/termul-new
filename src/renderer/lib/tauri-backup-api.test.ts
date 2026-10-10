import { beforeEach, describe, expect, it, vi } from 'vitest'

// In-memory filesystem: a path maps to `DIR` or to a file's contents.
const DIR = Symbol('dir')
const fsState = vi.hoisted(() => ({
  nodes: new Map<string, unknown>(),
  failRename: null as ((from: string) => boolean) | null
}))

function under(path: string): string[] {
  return [...fsState.nodes.keys()].filter((key) => key === path || key.startsWith(`${path}/`))
}

function addDir(path: string): void {
  const parts = path.split('/').filter(Boolean)
  for (let i = 1; i <= parts.length; i++) {
    fsState.nodes.set(`/${parts.slice(0, i).join('/')}`, DIR)
  }
}

function addFile(path: string, contents: string): void {
  addDir(path.slice(0, path.lastIndexOf('/')))
  fsState.nodes.set(path, contents)
}

vi.mock('@tauri-apps/api/path', () => ({ appDataDir: async () => '/app/data' }))

vi.mock('@tauri-apps/plugin-fs', () => ({
  mkdir: async (path: string) => addDir(path),
  readDir: async (path: string) => {
    if (fsState.nodes.get(path) !== DIR) throw new Error(`not a directory: ${path}`)
    return [...fsState.nodes.keys()]
      .filter((key) => key.startsWith(`${path}/`) && !key.slice(path.length + 1).includes('/'))
      .map((key) => {
        const isDirectory = fsState.nodes.get(key) === DIR
        return { name: key.slice(path.length + 1), isDirectory, isFile: !isDirectory }
      })
  },
  copyFile: async (from: string, to: string) => addFile(to, fsState.nodes.get(from) as string),
  readTextFile: async (path: string) => {
    const value = fsState.nodes.get(path)
    if (typeof value !== 'string') throw new Error(`no file: ${path}`)
    return value
  },
  writeTextFile: async (path: string, contents: string) => addFile(path, contents),
  remove: async (path: string) => {
    for (const key of under(path)) fsState.nodes.delete(key)
  },
  rename: async (from: string, to: string) => {
    if (fsState.failRename?.(from)) throw new Error(`rename refused: ${from}`)
    for (const key of under(from)) {
      const value = fsState.nodes.get(key)
      fsState.nodes.delete(key)
      fsState.nodes.set(to + key.slice(from.length), value)
    }
  },
  stat: async (path: string) => {
    if (!fsState.nodes.has(path)) throw new Error(`missing: ${path}`)
    return {}
  }
}))

vi.mock('@tauri-apps/plugin-store', () => {
  const values = new Map<string, unknown>()
  const store = {
    get: async (key: string) => values.get(key),
    set: async (key: string, value: unknown) => void values.set(key, value),
    delete: async (key: string) => void values.delete(key),
    save: async () => undefined
  }
  return { Store: { load: async () => store } }
})

import { BACKUP_EXCLUDED_ENTRIES, createBackup, restoreBackup } from './tauri-backup-api'

/** A profile with data a rollback needs next to caches it does not. */
function seedProfile(): void {
  addFile('/app/data/termul-data.json', '{"projects":["a"]}')
  addFile('/app/data/conversations/v2/c1.json', 'conversation')
  addFile('/app/data/memory-index/p1/index.sqlite3', 'huge index')
  addFile('/app/data/acp-npm-packages/claude/node_modules/sdk.js', 'agent package')
  addFile('/app/data/acp-registry-binaries/bin/agent', 'binary')
  addDir('/app/data/core-runtime')
  addFile('/app/data/versions/v0.15.0/meta.json', 'rollback')
}

describe('tauri-backup-api', () => {
  beforeEach(() => {
    fsState.nodes.clear()
    fsState.failRename = null
  })

  it('backs up user data but not caches that can be rebuilt or fetched again', async () => {
    seedProfile()

    const result = await createBackup()
    expect(result.success).toBe(true)
    if (!result.success) return
    const backup = result.data.path

    expect(fsState.nodes.get(`${backup}/termul-data.json`)).toBe('{"projects":["a"]}')
    expect(fsState.nodes.get(`${backup}/conversations/v2/c1.json`)).toBe('conversation')
    for (const name of BACKUP_EXCLUDED_ENTRIES) {
      expect(fsState.nodes.has(`${backup}/${name}`)).toBe(false)
    }
  })

  it('restores user data and keeps the current caches the backup left out', async () => {
    seedProfile()
    const backup = await createBackup()
    if (!backup.success) throw new Error(backup.error)
    addFile('/app/data/termul-data.json', '{"projects":["changed"]}')

    const result = await restoreBackup(backup.data.id)

    expect(result.success).toBe(true)
    expect(fsState.nodes.get('/app/data/termul-data.json')).toBe('{"projects":["a"]}')
    expect(fsState.nodes.get('/app/data/memory-index/p1/index.sqlite3')).toBe('huge index')
    expect(fsState.nodes.get('/app/data/acp-npm-packages/claude/node_modules/sdk.js')).toBe(
      'agent package'
    )
    expect(fsState.nodes.get('/app/data/versions/v0.15.0/meta.json')).toBe('rollback')
    // The backup being restored is still there to restore again.
    expect(fsState.nodes.has(`/app/data/backups/${backup.data.id}/backup-info.json`)).toBe(true)
    expect([...fsState.nodes.keys()].some((key) => key.startsWith('/app/old-userdata-'))).toBe(
      false
    )
  })

  it('keeps the replaced tree when a cache cannot be moved into the restored one', async () => {
    seedProfile()
    const backup = await createBackup()
    if (!backup.success) throw new Error(backup.error)
    fsState.failRename = (from) => from.endsWith('/memory-index')

    const result = await restoreBackup(backup.data.id)

    expect(result.success).toBe(true)
    const kept = [...fsState.nodes.keys()].find((key) =>
      /^\/app\/old-userdata-[^/]+\/memory-index\/p1\/index\.sqlite3$/.test(key)
    )
    expect(kept && fsState.nodes.get(kept)).toBe('huge index')
  })
})
