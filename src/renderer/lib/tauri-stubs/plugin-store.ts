/**
 * Browser stub for `@tauri-apps/plugin-store`.
 *
 * Web build only — aliased in by `vite.config.web.ts`. Backs the prior no-op
 * stub with `localStorage` so persistence (`persistenceApi`, editor layout,
 * projects, app settings, context-bar, terminal autosave, etc.) actually
 * round-trips on a web reload. Desktop uses the real Tauri plugin-store.
 *
 * - `load(path)` returns a Store namespaced by `path` so distinct stores never
 *   bleed into each other.
 * - `set` writes immediately (JSON-serialized); `save()` is a no-op (the web
 *   build has no deferred disk flush — data is already in `localStorage`).
 * - All ops catch `QuotaExceededError` / `SecurityError` / JSON parse failures
 *   and degrade silently (return `undefined` / empty, never throw to the UI).
 * - Guards `typeof localStorage` for SSR / non-browser hosts.
 *
 * The class shape and method signatures are preserved so every existing
 * consumer unblocks without per-consumer edits.
 */
import { acceptedBrandValues, LEGACY } from '@shared/brand'

/**
 * The prefix a write lands under — still the legacy one.
 *
 * A web install's projects, editor layout and panel sizes already sit under
 * this namespace, and FORBID-04 rules out emitting both spellings, so the flip
 * is a single deliberate edit (T-A08) rather than a dual-write window. Every
 * read below accepts both prefixes, which is what makes that edit safe.
 *
 * A function, not a `const`: the brand seam is overridable, and a prefix
 * captured at module scope would freeze before a test could move it.
 */
function writePrefix(): string {
  return LEGACY.storagePrefix
}

/**
 * Every namespace prefix a read must try, most-current first. Collapses to a
 * single entry until the canonical prefix diverges from the legacy one.
 */
function readPrefixes(): readonly string[] {
  return acceptedBrandValues('storagePrefix')
}

/** Escape `:` in a namespace so a namespace containing `::` (e.g. `a::b`)
 * can't make `clear('a')` prefix-match into `a::b`'s keys. Keys are NOT
 * escaped — they sit after the delimiter and may contain anything; `keys()`
 * already slices the prefix, so escaping only the namespace preserves key
 * values. Uses a control char (\u0001) that can't appear in a store path. */
function escapeNamespace(namespace: string): string {
  return namespace.replaceAll(':', '\u0001')
}

function storageKey(namespace: string, key: string): string {
  return `${writePrefix()}${escapeNamespace(namespace)}::${key}`
}

/** Every key a read must probe for `(namespace, key)`, most-current first. */
function readableStorageKeys(namespace: string, key: string): string[] {
  const suffix = `${escapeNamespace(namespace)}::${key}`
  return readPrefixes().map((prefix) => `${prefix}${suffix}`)
}

/** Every namespace scan prefix a read must walk, most-current first. */
function readableNamespacePrefixes(namespace: string): string[] {
  const suffix = `${escapeNamespace(namespace)}::`
  return readPrefixes().map((prefix) => `${prefix}${suffix}`)
}

function hasLocalStorage(): boolean {
  return typeof localStorage !== 'undefined'
}

export class Store {
  private readonly namespace: string
  /** Defaults passed to `load()` — re-applied by `reset()` (Tauri parity). */
  private readonly defaults: Record<string, unknown>

  private constructor(namespace: string, defaults: Record<string, unknown>) {
    this.namespace = namespace
    this.defaults = defaults
  }

  static async load(path: string, options?: unknown): Promise<Store> {
    // Tauri-faithful: `defaults` pre-populate keys that are absent on load. The
    // only current caller passes an empty `defaults` (a no-op), but apply the
    // semantics so the contract holds for any future caller.
    const rawDefaults = (options as { defaults?: Record<string, unknown> } | null | undefined)
      ?.defaults
    const defaults: Record<string, unknown> =
      rawDefaults && typeof rawDefaults === 'object' ? rawDefaults : {}
    const store = new Store(path, defaults)
    for (const [key, value] of Object.entries(defaults)) {
      if (!(await store.has(key))) {
        await store.set(key, value)
      }
    }
    return store
  }

  async get<T>(key: string): Promise<T | undefined> {
    if (!hasLocalStorage()) return undefined
    try {
      // First hit wins: the canonical namespace is authoritative once it
      // exists, and the legacy one is what a pre-rename install left behind.
      for (const stored of readableStorageKeys(this.namespace, key)) {
        const raw = localStorage.getItem(stored)
        if (raw == null) continue
        return JSON.parse(raw) as T
      }
      return undefined
    } catch {
      // Corrupt JSON / SecurityError (private mode) — degrade silently.
      return undefined
    }
  }

  async set(key: string, value: unknown): Promise<void> {
    if (!hasLocalStorage()) return
    try {
      localStorage.setItem(storageKey(this.namespace, key), JSON.stringify(value))
    } catch {
      // QuotaExceededError / SecurityError — drop the write silently. The UI
      // must never throw on persistence; reads return last-good or empty.
    }
  }

  async save(): Promise<void> {
    // No-op: `set` writes immediately to localStorage (no deferred disk flush
    // in the web build).
  }

  async delete(key: string): Promise<void> {
    if (!hasLocalStorage()) return
    try {
      // Every spelling `get` can see, or a delete would leave a legacy value
      // behind for the next read to resurrect.
      for (const stored of readableStorageKeys(this.namespace, key)) {
        localStorage.removeItem(stored)
      }
    } catch {
      // degrade silently
    }
  }

  async clear(): Promise<void> {
    if (!hasLocalStorage()) return
    try {
      const prefixes = readableNamespacePrefixes(this.namespace)
      // Collect first, then remove — mutating `localStorage` while iterating by
      // index would shift indices and skip entries.
      const toRemove: string[] = []
      for (let i = 0; i < localStorage.length; i++) {
        const stored = localStorage.key(i) ?? ''
        if (prefixes.some((prefix) => stored.startsWith(prefix))) toRemove.push(stored)
      }
      for (const k of toRemove) localStorage.removeItem(k)
    } catch {
      // degrade silently
    }
  }

  async reset(): Promise<void> {
    await this.clear()
    // Tauri-faithful parity: real `Store.reset()` clears AND re-applies the
    // `defaults` passed to `load()`. Without this, reset()-returns-defaults
    // callers silently diverge from desktop.
    for (const [key, value] of Object.entries(this.defaults)) {
      if (!(await this.has(key))) {
        await this.set(key, value)
      }
    }
  }

  async keys(): Promise<string[]> {
    if (!hasLocalStorage()) return []
    try {
      const prefixes = readableNamespacePrefixes(this.namespace)
      // A `Set`: the same key can exist under both prefixes, and a caller
      // iterating `keys()` would then read and report it twice.
      const keys = new Set<string>()
      for (let i = 0; i < localStorage.length; i++) {
        const stored = localStorage.key(i) ?? ''
        const prefix = prefixes.find((candidate) => stored.startsWith(candidate))
        if (prefix !== undefined) keys.add(stored.slice(prefix.length))
      }
      return Array.from(keys)
    } catch {
      return []
    }
  }

  async values<T>(): Promise<T[]> {
    const keys = await this.keys()
    const out: T[] = []
    for (const key of keys) {
      const v = await this.get<T>(key)
      if (v !== undefined) out.push(v)
    }
    return out
  }

  async entries<T>(): Promise<[string, T][]> {
    const keys = await this.keys()
    const out: [string, T][] = []
    for (const key of keys) {
      const v = await this.get<T>(key)
      if (v !== undefined) out.push([key, v])
    }
    return out
  }

  async length(): Promise<number> {
    return (await this.keys()).length
  }

  async has(key: string): Promise<boolean> {
    if (!hasLocalStorage()) return false
    try {
      return readableStorageKeys(this.namespace, key).some(
        (stored) => localStorage.getItem(stored) != null
      )
    } catch {
      return false
    }
  }
}
