/**
 * Filesystem API Singleton
 *
 * Exports a singleton instance of the FilesystemApi for use throughout the app.
 * This provides a consistent interface whether running under Electron or Tauri.
 *
 * Usage:
 *   import { filesystemApi } from '@/lib/filesystem-api'
 *   const result = await filesystemApi.readFile('/path/to/file')
 */

import { createTauriFilesystemApi, type PreviewFilesystemApi } from './tauri-filesystem-api'

/**
 * Singleton FilesystemApi instance
 *
 * Uses Tauri IPC implementation when running in Tauri context.
 * In the future, this could conditionally export an Electron implementation
 * based on build environment.
 */
export const filesystemApi: PreviewFilesystemApi = createTauriFilesystemApi()
