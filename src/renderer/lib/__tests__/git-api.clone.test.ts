import { beforeEach, describe, expect, it, vi } from 'vitest'

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }))

vi.mock('../tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: mockInvoke,
  Channel: class<T> {
    onmessage: (message: T) => void = () => undefined
  }
}))

import { type GitCloneProgress, gitApi } from '../git-api'

describe('gitApi clone', () => {
  beforeEach(() => {
    mockInvoke.mockReset().mockResolvedValue('/work/repo')
  })

  it('streams progress over a channel bound to the clone id', async () => {
    const onProgress = vi.fn()
    await expect(
      gitApi.clone('clone-1', 'https://github.com/a/repo.git', '/work', 'repo', onProgress)
    ).resolves.toBe('/work/repo')

    const [command, args] = mockInvoke.mock.calls[0]
    expect(command).toBe('git_clone')
    expect(args).toMatchObject({
      cloneId: 'clone-1',
      url: 'https://github.com/a/repo.git',
      parentDir: '/work',
      dirName: 'repo'
    })
    const line: GitCloneProgress = {
      line: 'Receiving objects:  5% (1/20)',
      phase: 'Receiving objects',
      percent: 5,
      transferred: null,
      speed: null
    }
    args.onProgress.onmessage(line)
    expect(onProgress).toHaveBeenCalledWith(line)
  })

  it('cancels by clone id', async () => {
    mockInvoke.mockResolvedValue(true)
    await expect(gitApi.cancelClone('clone-1')).resolves.toBe(true)
    expect(mockInvoke).toHaveBeenCalledWith('git_clone_cancel', { cloneId: 'clone-1' })
  })
})
