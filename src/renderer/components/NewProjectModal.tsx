import type { DetectedShells } from '@shared/types/ipc.types'
import type { ProjectTemplate } from '@shared/types/project-template.types'
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { ChevronDown, X } from 'lucide-react'
import { type KeyboardEvent, useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { Skeleton } from '@/components/ui/skeleton'
import { dialogApi, filesystemApi, gitApi, shellApi } from '@/lib/api'
import { availableColors, getColorClasses } from '@/lib/colors'
import { BUILT_IN_TEMPLATES, scaffoldProject } from '@/lib/project-templates'
import { isTauriContext } from '@/lib/tauri-runtime'
import { cn } from '@/lib/utils'
import { useDefaultProjectColor } from '@/stores/app-settings-store'
import type { EnvVariable, ProjectColor } from '@/types/project'

interface NewProjectModalProps {
  isOpen: boolean
  onClose: () => void
  onImportFromEditor?: () => void
  onCreateProject: (
    name: string,
    color: ProjectColor,
    path?: string,
    defaultShell?: string,
    envVars?: EnvVariable[]
  ) => void
}

const TEMPLATE_TRANSLATION_KEYS = {
  empty: { name: 'templates.empty.name', description: 'templates.empty.description' },
  node: { name: 'templates.node.name', description: 'templates.node.description' },
  rust: { name: 'templates.rust.name', description: 'templates.rust.description' },
  react: { name: 'templates.react.name', description: 'templates.react.description' },
  python: { name: 'templates.python.name', description: 'templates.python.description' }
} as const

function getTemplateTranslationKeys(id: string) {
  switch (id) {
    case 'node':
      return TEMPLATE_TRANSLATION_KEYS.node
    case 'rust':
      return TEMPLATE_TRANSLATION_KEYS.rust
    case 'react':
      return TEMPLATE_TRANSLATION_KEYS.react
    case 'python':
      return TEMPLATE_TRANSLATION_KEYS.python
    default:
      return TEMPLATE_TRANSLATION_KEYS.empty
  }
}

export function NewProjectModal({
  isOpen,
  onClose,
  onImportFromEditor,
  onCreateProject
}: NewProjectModalProps) {
  const { t } = useTranslation('projects')
  const reducedMotion = useReducedMotion() ?? false
  const defaultColor = useDefaultProjectColor() as ProjectColor
  const [name, setName] = useState('')
  const [selectedColor, setSelectedColor] = useState<ProjectColor>(defaultColor || 'blue')
  const [path, setPath] = useState('')
  const [shells, setShells] = useState<DetectedShells | null>(null)
  const [selectedShell, setSelectedShell] = useState<string>('')
  const [shellsLoading, setShellsLoading] = useState(true)
  const [selectedTemplate, setSelectedTemplate] = useState<ProjectTemplate>(BUILT_IN_TEMPLATES[0])
  const [isFolderEmpty, setIsFolderEmpty] = useState(false)
  const [initGit, setInitGit] = useState(false)

  // Platform-specific fallback shell
  const fallbackShell = navigator.platform.startsWith('Win') ? 'powershell' : 'bash'

  // Fetch available shells on mount
  useEffect(() => {
    const fetchShells = async () => {
      try {
        const result = await shellApi.getAvailableShells()
        if (result.success && result.data) {
          setShells(result.data)
          setSelectedShell(result.data.default?.name || fallbackShell)
        } else {
          // Detection failed - use fallback
          setSelectedShell(fallbackShell)
        }
      } catch (err) {
        console.error('Failed to detect shells:', err)
        setShells(null)
        setSelectedShell(fallbackShell)
      } finally {
        setShellsLoading(false)
      }
    }
    void fetchShells()
  }, [fallbackShell])

  // Check if chosen directory is empty
  useEffect(() => {
    const checkEmpty = async () => {
      const trimmed = path.trim()
      if (!trimmed) {
        setIsFolderEmpty(false)
        return
      }
      try {
        const result = await filesystemApi.readDirectory(trimmed)
        if (result.success && result.data) {
          setIsFolderEmpty(result.data.length === 0)
        } else {
          // If directory doesn't exist yet, treat it as empty
          setIsFolderEmpty(true)
        }
      } catch {
        setIsFolderEmpty(true)
      }
    }
    void checkEmpty()
  }, [path])

  // Reset form when modal opens (use defaults)
  useEffect(() => {
    if (isOpen) {
      setName('')
      setSelectedColor(defaultColor || 'blue')
      setPath('')
      setSelectedShell(shells?.default?.name || fallbackShell)
      setSelectedTemplate(BUILT_IN_TEMPLATES[0])
      setIsFolderEmpty(false)
      setInitGit(false)
    }
  }, [isOpen, defaultColor, shells?.default?.name, fallbackShell])

  // Handle Escape key to close modal
  useEffect(() => {
    if (!isOpen) return

    const handleEscape = (e: globalThis.KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    }

    window.addEventListener('keydown', handleEscape)
    return () => window.removeEventListener('keydown', handleEscape)
  }, [isOpen, onClose])

  const handleSelectTemplate = useCallback(
    (template: ProjectTemplate) => {
      setSelectedTemplate(template)
      if (template.defaultShell) {
        setSelectedShell(template.defaultShell)
      } else {
        setSelectedShell(shells?.default?.name || fallbackShell)
      }
    },
    [shells, fallbackShell]
  )

  const handleCreate = useCallback(() => {
    const trimmedName = name.trim()
    const trimmedPath = path.trim()

    if (trimmedName && trimmedPath) {
      // Use selected shell or fallback
      const shellToUse = selectedShell || fallbackShell

      const envVarsToPass: EnvVariable[] | undefined = selectedTemplate.envVars
        ? selectedTemplate.envVars.map((ev) => ({
            key: ev.key,
            value: ev.value,
            isSecret: ev.isSecret
          }))
        : undefined

      // Scaffold template files and initialize git asynchronously
      const runScaffoldAndGit = async () => {
        // Ensure root directory exists
        const dirResult = await filesystemApi.createDirectory(trimmedPath)
        if (!dirResult.success) {
          throw new Error(dirResult.error || t('operationFailed'))
        }

        let gitInitSucceeded = false
        // Initialize git repository if requested
        if (initGit) {
          try {
            await gitApi.init(trimmedPath)
            gitInitSucceeded = true
          } catch (err) {
            console.error('Git init failed during scaffolding:', err)
            // Continue even if git init fails, so files are still scaffolded
          }
        }

        // Scaffold template files
        if (selectedTemplate.id !== 'empty') {
          const res = await scaffoldProject(trimmedPath, trimmedName, selectedTemplate)
          if (!res.success) {
            throw new Error(res.error || t('operationFailed'))
          }
        }

        onCreateProject(trimmedName, selectedColor, trimmedPath, shellToUse, envVarsToPass)

        return { gitInitSucceeded }
      }

      const operationPromise = runScaffoldAndGit()
      toast.promise(operationPromise, {
        loading: initGit
          ? t('initializingAndScaffolding', {
              name: t(getTemplateTranslationKeys(selectedTemplate.id).name)
            })
          : t('scaffolding', { name: t(getTemplateTranslationKeys(selectedTemplate.id).name) }),
        success: (res) => {
          const templateName = t(getTemplateTranslationKeys(selectedTemplate.id).name)
          if (initGit) {
            return res.gitInitSucceeded
              ? t('scaffoldedGitSuccessfully', { name: templateName })
              : t('scaffoldedGitFailed', { name: templateName })
          }
          return t('scaffoldedSuccessfully', { name: templateName })
        },
        error: (err: Error) => t('setupFailed', { message: err.message })
      })

      onClose()
    }
  }, [
    name,
    selectedColor,
    path,
    selectedShell,
    fallbackShell,
    selectedTemplate,
    initGit,
    t,
    onCreateProject,
    onClose
  ])

  const handleBrowse = useCallback(async () => {
    const result = await dialogApi.selectDirectory()
    if (result.success) {
      setPath(result.data)
    }
  }, [])

  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.key === 'Enter' && name.trim() && path.trim()) {
        e.preventDefault()
        handleCreate()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        onClose()
      }
    },
    [name, path, handleCreate, onClose]
  )

  return (
    <AnimatePresence>
      {isOpen && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]"
          onClick={onClose}
        >
          <motion.div
            initial={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: reducedMotion ? 0 : -6 }}
            transition={{ duration: reducedMotion ? 0 : 0.15 }}
            className="flex max-h-[90vh] w-[520px] flex-col overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={handleKeyDown}
          >
            {/* Header */}
            <div className="flex h-9 shrink-0 items-center justify-between border-b border-border/70 px-3">
              <h3 className="text-xs font-semibold tracking-[-0.01em] text-foreground">
                {t('createNewProject')}
              </h3>
              <button
                onClick={onClose}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X size={14} />
              </button>
            </div>

            <div className="flex-1 space-y-3 overflow-y-auto p-4">
              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('projectTemplate')}
                </label>
                <div className="relative">
                  <select
                    value={selectedTemplate.id}
                    onChange={(e) => {
                      const tpl = BUILT_IN_TEMPLATES.find((t) => t.id === e.target.value)
                      if (tpl) handleSelectTemplate(tpl)
                    }}
                    className="h-8 w-full cursor-pointer appearance-none rounded-md border border-input/80 bg-secondary/35 px-2.5 pr-8 text-sm text-foreground outline-none focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                  >
                    {BUILT_IN_TEMPLATES.map((tpl) => (
                      <option key={tpl.id} value={tpl.id}>
                        {t(getTemplateTranslationKeys(tpl.id).name)}
                      </option>
                    ))}
                  </select>
                  <div className="pointer-events-none absolute inset-y-0 right-0 flex items-center px-2 text-muted-foreground">
                    <ChevronDown size={14} />
                  </div>
                </div>
                <p className="text-3xs text-muted-foreground mt-1 leading-snug">
                  {t(getTemplateTranslationKeys(selectedTemplate.id).description)}
                </p>
              </div>

              {selectedTemplate.envVars && selectedTemplate.envVars.length > 0 && (
                <div className="mt-2 rounded-md border border-border/80 bg-secondary/40 p-2.5">
                  <span className="text-3xs font-semibold text-muted-foreground block mb-1.5">
                    {t('includedEnvironmentVariables')}
                  </span>
                  <div className="flex flex-wrap gap-1.5">
                    {selectedTemplate.envVars.map((ev) => (
                      <span
                        key={ev.key}
                        className="rounded-md border border-border/80 bg-background px-2 py-0.5 font-mono text-3xs text-secondary-foreground"
                      >
                        {ev.key}={ev.value}
                      </span>
                    ))}
                  </div>
                </div>
              )}

              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('projectName')}
                </label>
                <input
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={t('namePlaceholder')}
                  className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                />
              </div>

              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('rootDirectory')}
                </label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={path}
                    onChange={(e) => setPath(e.target.value)}
                    placeholder={t('noDirectorySelected')}
                    className="h-8 flex-1 rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                  />
                  <button
                    onClick={handleBrowse}
                    className="inline-flex h-8 items-center rounded-md border border-border/80 bg-secondary/50 px-3 text-xs text-foreground transition-colors hover:bg-secondary focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  >
                    {t('browse')}
                  </button>
                </div>

                {isFolderEmpty && (
                  <div className="flex items-center gap-2 mt-2 px-1">
                    <input
                      type="checkbox"
                      id="init-git"
                      checked={initGit}
                      onChange={(e) => setInitGit(e.target.checked)}
                      className="rounded border-border text-primary bg-secondary focus:ring-primary h-3.5 w-3.5"
                    />
                    <label
                      htmlFor="init-git"
                      className="text-xs text-muted-foreground select-none cursor-pointer"
                    >
                      {t('initializeGit')}
                    </label>
                  </div>
                )}
              </div>

              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">{t('color')}</label>
                <div className="flex flex-wrap gap-2">
                  {availableColors.map((color) => {
                    const colors = getColorClasses(color)
                    return (
                      <button
                        key={color}
                        onClick={() => setSelectedColor(color)}
                        className={cn(
                          'size-6 rounded-sm transition-[opacity,box-shadow]',
                          colors.bg,
                          selectedColor === color
                            ? 'ring-1 ring-foreground/70 ring-offset-2 ring-offset-card'
                            : 'hover:opacity-80'
                        )}
                      />
                    )
                  })}
                </div>
              </div>

              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('defaultTerminal')}
                </label>
                {shellsLoading ? (
                  <Skeleton className="h-8 w-full rounded-md" />
                ) : (
                  <div className="relative">
                    <select
                      value={selectedShell}
                      onChange={(e) => setSelectedShell(e.target.value)}
                      className="h-8 w-full cursor-pointer appearance-none rounded-md border border-input/80 bg-secondary/35 px-2.5 pr-8 text-sm text-foreground outline-none focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                    >
                      {shells?.available && shells.available.length > 0 ? (
                        shells.available.map((shell) => (
                          <option key={shell.name} value={shell.name}>
                            {shell.displayName}
                          </option>
                        ))
                      ) : (
                        <option value="">{t('noShellsDetected')}</option>
                      )}
                    </select>
                    <div className="pointer-events-none absolute inset-y-0 right-0 flex items-center px-2 text-muted-foreground">
                      <ChevronDown size={14} />
                    </div>
                  </div>
                )}
              </div>

              {!isTauriContext() && (
                <p className="rounded-md bg-secondary/35 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
                  {t('webSessionNote')}
                </p>
              )}
            </div>

            {/* Footer */}
            <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-t border-border/70 bg-secondary/20 px-4">
              {onImportFromEditor ? (
                <button
                  type="button"
                  onClick={onImportFromEditor}
                  className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
                  data-testid="new-project-import-editors"
                >
                  {t('editorImport.title')}
                </button>
              ) : (
                <span />
              )}
              <div className="flex items-center gap-2">
                <button
                  onClick={onClose}
                  className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  {t('cancel')}
                </button>
                <button
                  onClick={handleCreate}
                  disabled={!name.trim() || !path.trim()}
                  className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
                >
                  {t('create')}
                </button>
              </div>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
