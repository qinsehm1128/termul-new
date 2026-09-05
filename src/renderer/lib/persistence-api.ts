/**
 * Persistence API Singleton
 *
 * Desktop (Tauri): `tauriPersistenceApi` — plugin-store file.
 * Web (se-server): `webPersistenceApi` — server-side JSON store over the
 * authenticated WS protocol (issue #613), so web-client state survives browser
 * switches / refreshes instead of per-browser localStorage.
 */

import type { PersistenceApi } from '@shared/types/ipc.types'
import { createTauriPersistenceApi } from './tauri-persistence-api'
import { isTauriContext } from './tauri-runtime'
import { createWebPersistenceApi } from './web-persistence-api'

export const persistenceApi: PersistenceApi = isTauriContext()
  ? createTauriPersistenceApi()
  : createWebPersistenceApi()

/**
 * User-imported color themes.
 *
 * Its own key rather than a field inside `settings/app`: importing or deleting
 * a theme would otherwise rewrite the whole settings blob, and every unrelated
 * settings write would carry the theme list along with it.
 */
export const CUSTOM_THEMES_KEY = 'themes/custom'
