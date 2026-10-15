import type { IpcResult } from '@shared/types/ipc.types'
import { Store } from '@tauri-apps/plugin-store'

const STORE_FILE = 'termul-data.json'
const DEBOUNCE_MS = 500
const CURRENT_VERSION = 1

interface PersistedStore<T> {
  _version: number
  data: T
}

type PendingWriteResolver = (result: IpcResult<void>) => void

interface PendingDebounceEntry<T = unknown> {
  timer: ReturnType<typeof setTimeout> | null
  data: T
  resolvers: PendingWriteResolver[]
  activeWrite: Promise<IpcResult<void>> | null
}

/**
 * Command history lives in one small file per project instead of the shared
 * store. `save()` rewrites a whole file, and agent prompts make history the
 * bulk of the data: with it in the shared file, every prompt sent rewrote
 * ~5 MB, and every unrelated write rewrote the history too.
 */
const COMMAND_HISTORY_KEY_PATTERN = /^projects\/([^/]+)\/command-history$/

function storeFileFor(key: string): string {
  const match = COMMAND_HISTORY_KEY_PATTERN.exec(key)
  return match ? `command-history/${match[1]}.json` : STORE_FILE
}

const storeInstances = new Map<string, Promise<Store>>()
/** Keys whose copy in the shared store is known to be gone. */
const legacyCopyCleared = new Set<string>()
const pendingDebounce = new Map<string, PendingDebounceEntry>()

function createSuccessResult(): IpcResult<void> {
  return { success: true, data: undefined }
}

function resolvePendingResolvers(resolvers: PendingWriteResolver[], result: IpcResult<void>): void {
  resolvers.forEach((resolve) => {
    resolve(result)
  })
}

function getStore(file: string = STORE_FILE): Promise<Store> {
  let store = storeInstances.get(file)
  if (!store) {
    store = Store.load(file, { autoSave: false, defaults: {} })
    // A failed load must not stick: the next call retries.
    store.catch(() => storeInstances.delete(file))
    storeInstances.set(file, store)
  }
  return store
}

/**
 * Drop the copy a key left in the shared store before it moved to its own
 * file. Runs once per key per session; the shared file is only rewritten when
 * there was something to remove.
 */
async function dropLegacyCopy(key: string): Promise<void> {
  if (storeFileFor(key) === STORE_FILE || legacyCopyCleared.has(key)) return
  const shared = await getStore()
  if ((await shared.get(key)) != null) {
    await shared.delete(key)
    await shared.save()
  }
  legacyCopyCleared.add(key)
}

async function persistVersionedData<T>(key: string, data: T): Promise<IpcResult<void>> {
  try {
    const store = await getStore(storeFileFor(key))
    const versioned: PersistedStore<T> = { _version: CURRENT_VERSION, data }
    await store.set(key, versioned)
    await store.save()
    await dropLegacyCopy(key)
    return createSuccessResult()
  } catch (err) {
    return { success: false, error: String(err), code: 'WRITE_ERROR' }
  }
}

function schedulePendingWrite(key: string, entry: PendingDebounceEntry): void {
  if (entry.timer) {
    clearTimeout(entry.timer)
  }

  entry.timer = setTimeout(() => {
    entry.timer = null
    void flushPendingEntry(key, entry)
  }, DEBOUNCE_MS)
}

async function flushPendingEntry(
  key: string,
  entry: PendingDebounceEntry
): Promise<IpcResult<void>> {
  if (entry.activeWrite) {
    const activeWriteResult = await entry.activeWrite

    if (entry.resolvers.length === 0) {
      if (entry.timer === null) {
        pendingDebounce.delete(key)
      }

      return activeWriteResult
    }
  }

  if (entry.resolvers.length === 0) {
    if (entry.timer === null && entry.activeWrite === null) {
      pendingDebounce.delete(key)
    }

    return createSuccessResult()
  }

  const dataToWrite = entry.data
  const resolvers = [...entry.resolvers]
  entry.resolvers = []

  const writePromise = persistVersionedData(key, dataToWrite)
    .then((result) => {
      resolvePendingResolvers(resolvers, result)
      return result
    })
    .finally(() => {
      entry.activeWrite = null

      if (entry.resolvers.length === 0 && entry.timer === null) {
        pendingDebounce.delete(key)
      }
    })

  entry.activeWrite = writePromise
  return writePromise
}

export const tauriPersistenceApi = {
  async read<T>(key: string): Promise<IpcResult<T>> {
    try {
      const file = storeFileFor(key)
      let raw = await (await getStore(file)).get<PersistedStore<T> | T>(key)
      // Not moved yet: still in the shared store until its first write here.
      if ((raw === null || raw === undefined) && file !== STORE_FILE) {
        raw = await (await getStore()).get<PersistedStore<T> | T>(key)
      }

      if (raw === null || raw === undefined) {
        return { success: false, error: `Key not found: ${key}`, code: 'KEY_NOT_FOUND' }
      }

      // Handle versioned data
      if (typeof raw === 'object' && raw !== null && '_version' in raw) {
        const versioned = raw as PersistedStore<T>
        return { success: true, data: versioned.data }
      }

      // Legacy data without version
      return { success: true, data: raw as T }
    } catch (err) {
      return { success: false, error: String(err), code: 'READ_ERROR' }
    }
  },

  async write<T>(key: string, data: T): Promise<IpcResult<void>> {
    return persistVersionedData(key, data)
  },

  async writeDebounced<T>(key: string, data: T): Promise<IpcResult<void>> {
    return new Promise((resolve) => {
      const existing = pendingDebounce.get(key)

      if (existing) {
        existing.data = data
        existing.resolvers.push(resolve)
        schedulePendingWrite(key, existing)
        return
      }

      const entry: PendingDebounceEntry<T> = {
        timer: null,
        data,
        resolvers: [resolve],
        activeWrite: null
      }

      pendingDebounce.set(key, entry)
      schedulePendingWrite(key, entry)
    })
  },

  async remove(key: string): Promise<IpcResult<void>> {
    try {
      const store = await getStore(storeFileFor(key))
      await store.delete(key)
      await store.save()
      await dropLegacyCopy(key)
      return createSuccessResult()
    } catch (err) {
      return { success: false, error: String(err), code: 'DELETE_ERROR' }
    }
  },

  // Alias for remove - matches PersistenceApi interface
  async delete(key: string): Promise<IpcResult<void>> {
    return this.remove(key)
  },

  async flushPendingWrites(): Promise<IpcResult<void>> {
    let firstFailure: IpcResult<void> | null = null

    for (const [key, entry] of Array.from(pendingDebounce.entries())) {
      if (entry.timer) {
        clearTimeout(entry.timer)
        entry.timer = null
      }

      const result = await flushPendingEntry(key, entry)
      if (!result.success && firstFailure === null) {
        firstFailure = result
      }
    }

    return firstFailure ?? createSuccessResult()
  }
}

/**
 * Factory function for consistency with other APIs
 */
export function createTauriPersistenceApi() {
  return tauriPersistenceApi
}

/**
 * @internal Testing only - reset the singleton store instance
 */
export function _resetStoreInstanceForTesting() {
  storeInstances.clear()
  legacyCopyCleared.clear()

  for (const entry of pendingDebounce.values()) {
    if (entry.timer) {
      clearTimeout(entry.timer)
    }
  }

  pendingDebounce.clear()
}
