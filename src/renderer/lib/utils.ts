import { type ClassValue, clsx } from 'clsx'
import { extendTailwindMerge } from 'tailwind-merge'

// The setting-driven row sizes (tailwind.config.ts) are font sizes; unregistered,
// tailwind-merge reads `text-sidebar-row` as a text color and drops it next to
// `text-foreground`.
const twMerge = extendTailwindMerge({
  extend: { classGroups: { 'font-size': [{ text: ['sidebar-row', 'tree-row'] }] } }
})

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}
