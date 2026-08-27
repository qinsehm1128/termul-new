export function pathBasename(path: string | null | undefined): string {
  if (!path) return ''
  return path.split(/[\\/]/).filter(Boolean).pop() ?? ''
}
