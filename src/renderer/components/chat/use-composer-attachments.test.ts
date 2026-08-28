import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MAX_INLINE_IMAGE_BYTES } from './chat-attachments'
import { useComposerAttachments } from './use-composer-attachments'

const { toastError, writeBytesToTempFile } = vi.hoisted(() => ({
  toastError: vi.fn(),
  writeBytesToTempFile: vi.fn(async () => '/tmp/termul/pasted.png')
}))

vi.mock('sonner', () => ({ toast: { error: toastError } }))
vi.mock('@/lib/tauri-runtime', () => ({ isTauriContext: () => true }))
vi.mock('@/lib/attachment-api', () => ({ readAttachmentBytes: vi.fn() }))
vi.mock('@/lib/attachment-temp-cleanup', () => ({ deleteTempFile: vi.fn() }))
vi.mock('@/lib/composer-attachments-io', () => ({
  pickAttachmentFilesBrowser: vi.fn(),
  pickAttachmentPaths: vi.fn(),
  readClipboardRgbaImage: vi.fn(),
  writeBytesToTempFile
}))

function imageFile(bytes: number): File {
  const file = new File([new Uint8Array(bytes)], 'shot.png', { type: 'image/png' })
  // jsdom's File does not implement `arrayBuffer()`; supply the real semantics
  // so the temp-link path under test can read the bytes.
  Object.defineProperty(file, 'arrayBuffer', {
    value: async (): Promise<ArrayBuffer> => new ArrayBuffer(bytes)
  })
  return file
}

beforeEach(() => {
  toastError.mockClear()
  writeBytesToTempFile.mockClear()
})

describe('composer image attachments vs the persisted record bound', () => {
  it('inlines an image that fits the inline budget', async () => {
    const { result } = renderHook(() =>
      useComposerAttachments({ imageCapable: true, embedCapable: false, disabled: false })
    )

    await act(async () => {
      await result.current.addFiles([imageFile(1024)])
    })

    await waitFor(() => expect(result.current.attachments).toHaveLength(1))
    expect(result.current.attachments[0].kind).toBe('image')
    expect(toastError).not.toHaveBeenCalled()
  })

  it('links an oversized image by path instead of inlining or rejecting it', async () => {
    // Inlining past MAX_INLINE_IMAGE_BYTES makes the backend reject the whole
    // prompt (EVENT_DELIVERY_FAILED), which the user experiences as "the message
    // will not send". The image must degrade to a resource_link, not a toast.
    const { result } = renderHook(() =>
      useComposerAttachments({ imageCapable: true, embedCapable: false, disabled: false })
    )

    await act(async () => {
      await result.current.addFiles([imageFile(MAX_INLINE_IMAGE_BYTES + 1)])
    })

    await waitFor(() => expect(result.current.attachments).toHaveLength(1))
    const attached = result.current.attachments[0]
    expect(attached.kind).toBe('file-ref')
    expect(writeBytesToTempFile).toHaveBeenCalledTimes(1)
    expect(toastError).not.toHaveBeenCalled()
  })
})
