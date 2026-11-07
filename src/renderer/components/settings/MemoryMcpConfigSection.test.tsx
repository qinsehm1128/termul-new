import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryMcpConfigSection } from './MemoryMcpConfigSection'

const mocks = vi.hoisted(() => ({
  desktop: true,
  invocation: vi.fn(),
  writeText: vi.fn(),
  toast: vi.fn(),
  log: vi.fn()
}))

vi.mock('react-i18next', () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => mocks.desktop }))
vi.mock('@/lib/api', () => ({
  memoryIndexApi: { universalMcpInvocation: mocks.invocation },
  clipboardApi: { writeText: mocks.writeText }
}))
vi.mock('@/hooks/use-toast', () => ({ toast: mocks.toast }))
vi.mock('@/lib/log-api', () => ({ logFrontendError: mocks.log }))

const invocation = [
  'C:\\Program Files\\Se\\se-memory.exe',
  '--root',
  'C:\\My Projects\\notes',
  '--readonly'
]
const json = JSON.stringify(
  { mcpServers: { 'se-memory': { command: invocation[0], args: invocation.slice(1) } } },
  null,
  2
)
const toml =
  '[mcp_servers.se-memory]\ncommand = "C:\\\\Program Files\\\\Se\\\\se-memory.exe"\nargs = ["--root", "C:\\\\My Projects\\\\notes", "--readonly"]\n'

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function click(label: string) {
  fireEvent.click(screen.getByRole('button', { name: `memory.${label}` }))
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.desktop = true
  mocks.invocation.mockResolvedValue(invocation)
  mocks.writeText.mockResolvedValue({ success: true })
  mocks.log.mockResolvedValue(undefined)
})
afterEach(cleanup)

describe('MemoryMcpConfigSection compatibility and export safety', () => {
  it('requests nothing until interaction and preserves memory-only JSON argv exactly', async () => {
    const { container } = render(<MemoryMcpConfigSection />)
    expect(mocks.invocation).not.toHaveBeenCalled()
    expect(mocks.log).not.toHaveBeenCalled()
    click('preview')
    await waitFor(() => expect(container.querySelector('pre')?.textContent).toBe(json))
    expect(mocks.writeText).not.toHaveBeenCalled()
    click('copy')
    await waitFor(() => expect(mocks.toast).toHaveBeenCalledWith({ title: 'memory.copied' }))
    expect(mocks.invocation).toHaveBeenCalledTimes(2)
    expect(mocks.writeText).toHaveBeenCalledWith(json)
    expect(json).not.toContain('se-mcp')
    expect(json).not.toContain('ssh')
  })

  it('clears an existing preview on format switch and exports exact memory-only TOML', async () => {
    const { container } = render(<MemoryMcpConfigSection />)
    click('preview')
    await waitFor(() => expect(container.querySelector('pre')?.textContent).toBe(json))
    click('formatToml')
    expect(container.querySelector('pre')).toBeNull()
    click('copy')
    await waitFor(() => expect(mocks.writeText).toHaveBeenCalledWith(toml))
    expect(container.querySelector('pre')?.textContent).toBe(toml)
  })

  it.each([
    null,
    []
  ])('does not copy a previous preview when invocation is unavailable (%j)', async (missing) => {
    const { container } = render(<MemoryMcpConfigSection />)
    click('preview')
    await waitFor(() => expect(container.querySelector('pre')).not.toBeNull())
    mocks.invocation.mockResolvedValueOnce(missing)
    click('copy')
    await waitFor(() =>
      expect(mocks.toast).toHaveBeenCalledWith({
        title: 'memory.unavailable',
        variant: 'destructive'
      })
    )
    expect(container.querySelector('pre')).toBeNull()
    expect(mocks.writeText).not.toHaveBeenCalled()
    expect(mocks.toast).not.toHaveBeenCalledWith({ title: 'memory.copied' })
  })

  it('handles thrown invocation failures without old preview fallback or raw error disclosure', async () => {
    const { container } = render(<MemoryMcpConfigSection />)
    click('preview')
    await waitFor(() => expect(container.querySelector('pre')).not.toBeNull())
    mocks.invocation.mockRejectedValueOnce(new Error('TOKEN_SENTINEL C:\\private\\secret --token'))
    click('copy')
    await waitFor(() =>
      expect(mocks.toast).toHaveBeenCalledWith({
        title: 'memory.copyFailed',
        variant: 'destructive'
      })
    )
    expect(mocks.writeText).not.toHaveBeenCalled()
    expect(container.querySelector('pre')).toBeNull()
    expect(JSON.stringify([mocks.log.mock.calls, mocks.toast.mock.calls])).not.toContain(
      'TOKEN_SENTINEL'
    )
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('Program Files')
    expect(JSON.stringify(mocks.log.mock.calls)).not.toContain('--readonly')
  })

  it.each([
    'result',
    'throw'
  ])('does not announce success after clipboard failure (%s)', async (failure) => {
    if (failure === 'result')
      mocks.writeText.mockResolvedValueOnce({
        success: false,
        error: { message: 'TOKEN_SENTINEL' }
      })
    else mocks.writeText.mockRejectedValueOnce(new Error('TOKEN_SENTINEL'))
    render(<MemoryMcpConfigSection />)
    click('copy')
    await waitFor(() =>
      expect(mocks.toast).toHaveBeenCalledWith({
        title: 'memory.copyFailed',
        variant: 'destructive'
      })
    )
    expect(mocks.toast).not.toHaveBeenCalledWith({ title: 'memory.copied' })
    expect(JSON.stringify([mocks.log.mock.calls, mocks.toast.mock.calls])).not.toContain(
      'TOKEN_SENTINEL'
    )
  })

  it('ignores an outdated invocation after switching formats while the new request succeeds', async () => {
    const old = deferred<string[]>()
    mocks.invocation.mockReturnValueOnce(old.promise)
    const { container } = render(<MemoryMcpConfigSection />)
    click('copy')
    click('formatToml')
    click('preview')
    await waitFor(() => expect(container.querySelector('pre')?.textContent).toBe(toml))
    await act(async () => old.resolve(invocation))
    expect(container.querySelector('pre')?.textContent).toBe(toml)
    expect(mocks.writeText).not.toHaveBeenCalled()
    expect(mocks.toast).not.toHaveBeenCalled()
  })

  it('ignores stale invocation rejection after switching formats', async () => {
    const old = deferred<string[]>()
    mocks.invocation.mockReturnValueOnce(old.promise)
    render(<MemoryMcpConfigSection />)
    click('preview')
    click('formatToml')
    await act(async () => old.reject(new Error('TOKEN_SENTINEL')))
    expect(mocks.toast).not.toHaveBeenCalled()
    expect(mocks.log).toHaveBeenCalledTimes(1)
  })

  it('does not copy or toast after unmount while loading invocation', async () => {
    const old = deferred<string[]>()
    mocks.invocation.mockReturnValueOnce(old.promise)
    const { unmount } = render(<MemoryMcpConfigSection />)
    click('copy')
    unmount()
    await act(async () => old.resolve(invocation))
    expect(mocks.writeText).not.toHaveBeenCalled()
    expect(mocks.toast).not.toHaveBeenCalled()
  })

  it.each([
    'format',
    'unmount'
  ])('suppresses stale clipboard completion after %s', async (invalidate) => {
    const write = deferred<{ success: boolean }>()
    mocks.writeText.mockReturnValueOnce(write.promise)
    const { unmount } = render(<MemoryMcpConfigSection />)
    click('copy')
    await waitFor(() => expect(mocks.writeText).toHaveBeenCalledTimes(1))
    if (invalidate === 'format') click('formatToml')
    else unmount()
    await act(async () => write.resolve({ success: true }))
    expect(mocks.toast).not.toHaveBeenCalled()
  })

  it('marks browser exports desktop-only and never calls native APIs', async () => {
    mocks.desktop = false
    render(<MemoryMcpConfigSection />)
    expect(screen.getByText('export.desktopOnly')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'memory.preview' }).hasAttribute('disabled')).toBe(
      true
    )
    expect(screen.getByRole('button', { name: 'memory.copy' }).hasAttribute('disabled')).toBe(true)
    click('preview')
    click('copy')
    click('formatToml')
    expect(mocks.invocation).not.toHaveBeenCalled()
    expect(mocks.writeText).not.toHaveBeenCalled()
    expect(mocks.log).not.toHaveBeenCalled()
  })
})
