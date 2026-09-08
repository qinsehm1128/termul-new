import { describe, expect, it } from 'vitest'
import { isManualOrderingEnabled, sortProjects } from './project-sort'

function p(name: string, id = name): { id: string; name: string } {
  return { id, name }
}

const names = (list: readonly { name: string }[]): string[] => list.map((item) => item.name)

describe('sortProjects', () => {
  describe('manual mode', () => {
    it('leaves the drag order alone', () => {
      const list = [p('zeta'), p('alpha'), p('beta')]
      expect(names(sortProjects(list, 'manual'))).toEqual(['zeta', 'alpha', 'beta'])
    })

    it('returns the same reference so memoised callers do no work', () => {
      const list = [p('zeta'), p('alpha')]
      expect(sortProjects(list, 'manual')).toBe(list)
    })
  })

  describe('name mode', () => {
    it('sorts alphabetically', () => {
      const list = [p('zeta-web'), p('alpha-service'), p('beta-tools')]
      expect(names(sortProjects(list, 'name'))).toEqual(['alpha-service', 'beta-tools', 'zeta-web'])
    })

    it('does not copy uppercase names into a separate block', () => {
      // Codepoint order would put every capital ahead of every lowercase,
      // splitting names a user reads as adjacent.
      const list = [p('beta'), p('Alpha'), p('Beta'), p('alpha')]
      const sorted = names(sortProjects(list, 'name'))
      expect(sorted.slice(0, 2).map((n) => n.toLowerCase())).toEqual(['alpha', 'alpha'])
      expect(sorted.slice(2).map((n) => n.toLowerCase())).toEqual(['beta', 'beta'])
    })

    it('orders embedded numbers naturally', () => {
      const list = [p('项目10'), p('项目9'), p('项目1')]
      expect(names(sortProjects(list, 'name'))).toEqual(['项目1', '项目9', '项目10'])
    })

    it('orders ascii-numbered names naturally too', () => {
      const list = [p('svc-10'), p('svc-2'), p('svc-1')]
      expect(names(sortProjects(list, 'name'))).toEqual(['svc-1', 'svc-2', 'svc-10'])
    })

    it('does not mutate the input', () => {
      const list = [p('zeta'), p('alpha')]
      sortProjects(list, 'name')
      expect(names(list)).toEqual(['zeta', 'alpha'])
    })

    it('breaks ties on id so equal names keep a stable order', () => {
      // Two projects can legitimately share a name. Without a tiebreak their
      // relative order could flip between renders and the list would jitter.
      const list = [p('dup', 'id-b'), p('dup', 'id-a')]
      const once = sortProjects(list, 'name').map((item) => item.id)
      const again = sortProjects([...list].reverse(), 'name').map((item) => item.id)
      expect(once).toEqual(['id-a', 'id-b'])
      expect(again).toEqual(once)
    })

    it('handles an empty list', () => {
      expect(sortProjects([], 'name')).toEqual([])
    })
  })
})

describe('isManualOrderingEnabled', () => {
  it('allows dragging only in manual mode', () => {
    expect(isManualOrderingEnabled('manual')).toBe(true)
    expect(isManualOrderingEnabled('name')).toBe(false)
  })
})
