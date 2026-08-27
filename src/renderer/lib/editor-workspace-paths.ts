/** Normalize a project root for duplicate detection across editors. */
export function normalizeProjectPath(path: string): string {
  const trimmed = path.trim()
  if (!trimmed) return ''
  const unified = trimmed.replace(/\\/g, '/')
  if (unified === '/') return unified
  return unified.replace(/\/+$/, '').toLowerCase()
}
