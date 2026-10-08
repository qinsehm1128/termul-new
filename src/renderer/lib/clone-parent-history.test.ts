import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockRead, mockWrite } = vi.hoisted(() => ({ mockRead: vi.fn(), mockWrite: vi.fn() }))
vi.mock('@/lib/api', () => ({ persistenceApi: { read: mockRead, write: mockWrite } }))

import {
  CLONE_PARENT_HISTORY_LIMIT,
  loadCloneParentHistory,
  rememberCloneParent,
  withCloneParent
} from './clone-parent-history'

describe('clone parent history', () => {
  beforeEach(() => {
    mockRead.mockReset()
    mockWrite.mockReset().mockResolvedValue({ success: true })
  })

  it('puts the latest folder first without duplicates, up to the limit', () => {
    const many = Array.from({ length: CLONE_PARENT_HISTORY_LIMIT }, (_, i) => `/d${i}`)
    const next = withCloneParent(many, '/d3/')
    expect(next[0]).toBe('/d3')
    expect(next.filter((d) => d === '/d3')).toHaveLength(1)
    expect(withCloneParent(many, '/new')).toHaveLength(CLONE_PARENT_HISTORY_LIMIT)
  })

  it('ignores anything that is not a list of folders', async () => {
    mockRead.mockResolvedValue({ success: true, data: ['/a', 3, '', '/b'] })
    expect(await loadCloneParentHistory()).toEqual(['/a', '/b'])
    mockRead.mockResolvedValue({ success: true, data: { nope: 1 } })
    expect(await loadCloneParentHistory()).toEqual([])
  })

  it('persists the updated list', async () => {
    mockRead.mockResolvedValue({ success: true, data: ['/a'] })
    await rememberCloneParent('/b')
    expect(mockWrite).toHaveBeenCalledWith('projects/clone-parent-dirs', ['/b', '/a'])
  })
})
