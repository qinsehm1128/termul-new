import { FileEdit, Save, X } from 'lucide-react'
import { useCallback, useState } from 'react'
import { toast } from 'sonner'
import { useSshTranslation } from '@/hooks/use-ssh-translation'
import { sshApi } from '@/lib/api'
import { cn } from '@/lib/utils'
import { useSSHActions, useSSHEditorContent, useSSHEditorFile } from '@/stores/ssh-store'

interface SSHFileEditorProps {
  connectionId: string
}

export function SSHFileEditor({ connectionId }: SSHFileEditorProps): React.JSX.Element {
  const t = useSshTranslation()
  const { setEditingFile: setStoreFile, setEditingContent: setStoreContent } = useSSHActions()
  const editingFile = useSSHEditorFile()
  const editingContent = useSSHEditorContent()
  const [isSaving, setIsSaving] = useState(false)
  const [saveAnimating, setSaveAnimating] = useState(false)
  const [confirmClose, setConfirmClose] = useState(false)

  const isDirty = editingFile !== null && editingContent !== editingFile.originalContent

  const handleSave = useCallback(async () => {
    if (!editingFile || !connectionId) return
    setIsSaving(true)
    setSaveAnimating(true)
    try {
      const result = await sshApi.sftpWriteFile(connectionId, editingFile.path, editingContent)
      if (result.success) {
        setStoreFile({ ...editingFile, originalContent: editingContent })
        setConfirmClose(false)
        toast.success(t('fileEditor.saved', { name: editingFile.name }))
        setTimeout(() => setSaveAnimating(false), 600)
      } else {
        toast.error(t('fileEditor.saveFailed', { error: result.error }))
        setSaveAnimating(false)
      }
    } catch (error) {
      toast.error(
        t('fileEditor.saveFailed', {
          error: error instanceof Error ? error.message : String(error)
        })
      )
      setSaveAnimating(false)
    } finally {
      setIsSaving(false)
    }
  }, [editingFile, connectionId, editingContent, setStoreFile, t])

  const handleClose = useCallback(() => {
    if (isDirty) setConfirmClose(true)
    else setStoreFile(null)
  }, [isDirty, setStoreFile])

  if (!editingFile) return <></>

  return (
    <>
      <div className="flex-1 flex flex-col">
        <div className="flex h-8 items-center justify-between border-b border-border/70 bg-secondary/20 px-3">
          <div className="flex items-center gap-2">
            <FileEdit className="h-3 w-3 text-muted-foreground" />
            <span className="text-2xs font-mono text-muted-foreground">{editingFile.name}</span>
          </div>
          <div className="flex items-center gap-1">
            <button
              onClick={handleSave}
              disabled={isSaving}
              className={cn(
                'rounded-md p-1 transition-colors',
                saveAnimating
                  ? 'bg-success/20 text-success'
                  : isDirty
                    ? 'bg-warning/20 text-warning'
                    : 'text-muted-foreground hover:bg-secondary hover:text-foreground'
              )}
              title={t('actions.save')}
            >
              <Save className={cn('h-3 w-3', saveAnimating && 'animate-pulse')} />
            </button>
            <button
              onClick={handleClose}
              className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
              title={t('actions.close')}
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        </div>
        <textarea
          value={editingContent}
          onChange={(e) => setStoreContent(e.target.value)}
          className="w-full flex-1 resize-none bg-card p-3 font-mono text-xs text-foreground outline-none"
          spellCheck={false}
        />
      </div>

      {confirmClose && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]">
          <div className="w-[340px] overflow-hidden rounded-md border border-border/80 bg-card p-4 shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]">
            <h3 className="mb-2 text-xs font-semibold tracking-[-0.01em] text-foreground">
              {t('fileEditor.unsavedTitle')}
            </h3>
            <p className="mb-4 text-xs text-muted-foreground">
              {t('fileEditor.unsavedMessage', { name: editingFile.name })}
            </p>
            <div className="flex justify-end gap-2">
              <button
                onClick={() => setConfirmClose(false)}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('fileEditor.continueEditing')}
              </button>
              <button
                onClick={handleSave}
                disabled={isSaving}
                className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
              >
                {t('actions.save')}
              </button>
              <button
                onClick={() => {
                  setStoreFile(null)
                  setConfirmClose(false)
                }}
                className="inline-flex h-8 items-center rounded-md bg-destructive px-3 text-xs font-medium text-destructive-foreground transition-colors hover:bg-destructive/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('actions.discard')}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}
