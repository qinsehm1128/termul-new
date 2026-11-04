import type { HostWhen } from '@shared/types/plugin-host.types'

export type ContributionContext = Record<keyof HostWhen, boolean>

/** A finite AND, never an expression interpreter. Unknown keys fail closed. */
export function matchesHostWhen(when: HostWhen | undefined, context: ContributionContext): boolean {
  return Object.entries(when ?? {}).every(
    ([key, value]) =>
      Object.hasOwn(context, key) &&
      typeof value === 'boolean' &&
      context[key as keyof HostWhen] === value
  )
}
