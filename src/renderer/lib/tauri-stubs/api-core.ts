import { tauriUnavailable } from './unavailable'

export type InvokeArgs = Record<string, unknown> | undefined

export async function invoke(_cmd: string, _args?: InvokeArgs): Promise<never> {
  return tauriUnavailable('Tauri invoke')
}

/** Web builds keep local paths unchanged because they do not use Tauri assets. */
export function convertFileSrc(filePath: string, _protocol = 'asset'): string {
  return filePath
}

/** Minimal Channel shim — PTY/binary channel surface used by tauri-terminal-api. */
export class Channel<T = unknown> {
  onmessage: ((data: T) => void) | null = null
}
