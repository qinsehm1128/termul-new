import type { DetectedShells } from '@shared/types/ipc.types'
import { LayoutGroup, motion, Reorder } from 'framer-motion'
import {
  AlertTriangle,
  Archive,
  ChevronDown,
  ChevronRight,
  Copy,
  Edit2,
  Folder,
  FolderInput,
  FolderOpen,
  FolderPlus,
  GitBranch,
  Palette,
  Plus,
  RotateCcw,
  Search,
  Settings,
  Terminal,
  Trash2,
  X
} from 'lucide-react'
import { type KeyboardEvent, memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { CollapseExpandMotion } from '@/components/ui/collapse-expand-motion'
import {
  ContextMenu,
  ContextMenuCheckboxItem,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuRadioGroup,
  ContextMenuRadioItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger
} from '@/components/ui/context-menu'
import { MonochromeSpinner } from '@/components/ui/monochrome-spinner'
import { Skeleton } from '@/components/ui/skeleton'
import { toast } from '@/hooks/use-toast'
import { useWorktreeReconciler } from '@/hooks/use-worktree-reconciler'
import { clipboardApi, dialogApi, shellApi } from '@/lib/api'
import { availableColors, getColorClasses } from '@/lib/colors'
import { filterProjects, shouldShowProjectSearch } from '@/lib/project-filter'
import { cn } from '@/lib/utils'
import { useProjectsWithActiveAgentChat } from '@/stores/acp-store'
import { isRejectedMove, useFileExplorerStore } from '@/stores/file-explorer-store'
import { useProjectActions, useProjectStore } from '@/stores/project-store'
import { useSSHPanelVisible } from '@/stores/ssh-panel-store'
import { useProjectsWithActivity, useProjectsWithErrors } from '@/stores/terminal-store'
import type { Project, ProjectColor } from '@/types/project'
import { ColorPickerPopover } from './ColorPickerPopover'
import { ConfirmDialog } from './ConfirmDialog'
import { NewGroupModal } from './NewGroupModal'
import { NewWorktreeModal } from './NewWorktreeModal'
import { SSHPanel } from './ssh/SSHPanel'

interface ColorPickerState {
  isOpen: boolean
  x: number
  y: number
  targetId: string
  targetType: 'project' | 'group'
}

interface DeleteConfirmState {
  isOpen: boolean
  projectId: string
  projectName: string
}

interface SettingsDialogState {
  isOpen: boolean
  projectId: string
}

interface NewWorktreeModalState {
  isOpen: boolean
  projectId: string
}

interface ProjectSidebarProps {
  projects: Project[]
  activeProjectId: string
  onSelectProject: (id: string) => void
  onOpenProjectTerminal: (id: string) => void
  onSelectGroup?: (id: string) => void
  onNewProject: () => void
  onImportFromEditor?: (groupId?: string) => void
  onUpdateProject: (id: string, updates: Partial<Project>) => void
  onDeleteProject: (id: string) => void
  onArchiveProject: (id: string) => void
  onRestoreProject: (id: string) => void
  onReorderProjects: (projectIds: string[]) => void
  onSSHConnect?: (profileId: string) => void
  onSelectSSHProfile?: (profileId: string) => void
  activeSSHProfileId?: string | null
}

export function ProjectSidebar({
  projects,
  activeProjectId,
  onSelectProject,
  onOpenProjectTerminal,
  onSelectGroup,
  onNewProject,
  onImportFromEditor,
  onUpdateProject,
  onDeleteProject,
  onArchiveProject,
  onRestoreProject,
  onReorderProjects,
  onSSHConnect,
  onSelectSSHProfile,
  activeSSHProfileId
}: ProjectSidebarProps): React.JSX.Element {
  const { t } = useTranslation('projects')
  const navigate = useNavigate()
  const {
    selectProject,
    selectGroup,
    addProject,
    addGroup,
    removeGroup,
    renameGroup,
    toggleGroupCollapse,
    moveProjectToGroup,
    reorderGroups,
    reorderProjectInGroup,
    updateGroup
  } = useProjectActions()
  const storeGroups = useProjectStore((state) => state.groups)
  const activeGroupId = useProjectStore((state) => state.activeGroupId)
  const groups = useMemo(() => storeGroups ?? [], [storeGroups])
  const handleSelectGroup = onSelectGroup ?? selectGroup

  // Reconcile stored worktrees against actual git state (detects orphaned entries)
  useWorktreeReconciler(activeProjectId)

  // Show archived toggle state
  const [showArchived, setShowArchived] = useState(false)

  // Group management states
  const [editingGroupId, setEditingGroupId] = useState<string | null>(null)
  const [editGroupName, setEditGroupName] = useState('')
  const [newGroupModal, setNewGroupModal] = useState<{
    isOpen: boolean
    projectIdToMove?: string
  }>({ isOpen: false })
  const [groupDeleteConfirm, setGroupDeleteConfirm] = useState({
    isOpen: false,
    groupId: '',
    groupName: '',
    deleteProjects: false
  })

  // Last right-click coordinates, captured so the `ColorPickerPopover` (a
  // Popover, not a Radix context menu — kept as-is per spec) can open near the
  // pointer after a "Change Color" menu item is selected. The Radix
  // `<ContextMenuTrigger>` wrapping each row owns menu open/positioning.
  const contextMenuPosRef = useRef({ x: 0, y: 0 })

  const [activeDragOverGroupId, setActiveDragOverGroupId] = useState<string | null>(null)
  const activeDragOverGroupIdRef = useRef<string | null>(null)

  // Project search/filter query
  const [searchQuery, setSearchQuery] = useState('')
  const searchInputRef = useRef<HTMLInputElement>(null)

  // Inline editing state
  const [editingId, setEditingId] = useState<string | null>(null)
  const [editName, setEditName] = useState('')

  // Color picker state
  const [colorPicker, setColorPicker] = useState<ColorPickerState>({
    isOpen: false,
    x: 0,
    y: 0,
    targetId: '',
    targetType: 'project'
  })

  const handleOpenColorPicker = useCallback(
    (targetId: string, targetType: 'project' | 'group', x: number, y: number): void => {
      setColorPicker({
        isOpen: true,
        x,
        y,
        targetId,
        targetType
      })
    },
    []
  )

  const closeColorPicker = useCallback((): void => {
    setColorPicker((prev) => ({ ...prev, isOpen: false }))
  }, [])

  const handleColorChange = useCallback(
    (color: ProjectColor): void => {
      if (colorPicker.targetId) {
        if (colorPicker.targetType === 'project') {
          onUpdateProject(colorPicker.targetId, { color })
        } else if (colorPicker.targetType === 'group') {
          updateGroup(colorPicker.targetId, { color })
        }
      }
    },
    [colorPicker.targetId, colorPicker.targetType, onUpdateProject, updateGroup]
  )

  // Delete confirmation state
  const [deleteConfirm, setDeleteConfirm] = useState<DeleteConfirmState>({
    isOpen: false,
    projectId: '',
    projectName: ''
  })

  // Settings dialog state
  const [settingsDialog, setSettingsDialog] = useState<SettingsDialogState>({
    isOpen: false,
    projectId: ''
  })

  // New worktree modal state
  const [newWorktreeModal, setNewWorktreeModal] = useState<NewWorktreeModalState>({
    isOpen: false,
    projectId: ''
  })

  // Settings form state
  const [settingsName, setSettingsName] = useState('')
  const [settingsPath, setSettingsPath] = useState('')
  const [settingsShell, setSettingsShell] = useState('')
  const [settingsColor, setSettingsColor] = useState<ProjectColor>('blue')
  const [settingsPathLoading, setSettingsPathLoading] = useState(false)

  // Available shells state
  const [availableShells, setAvailableShells] = useState<DetectedShells | null>(null)

  // Fetch available shells on mount
  useEffect(() => {
    const fetchShells = async () => {
      try {
        const result = await shellApi.getAvailableShells()
        if (result.success) {
          setAvailableShells(result.data)
        }
      } catch {
        // Ignore errors
      }
    }
    void fetchShells()
  }, [])

  // Optimized subscription: only re-render sidebar if which projects have activity changes.
  // This prevents re-renders when terminal text output changes.
  const [projectActivityIds, projectErrorIds] = [useProjectsWithActivity(), useProjectsWithErrors()]
  const agentChatActivityIds = useProjectsWithActiveAgentChat()
  const projectHasActivity = useCallback(
    (projectId: string) =>
      projectActivityIds.includes(projectId) || agentChatActivityIds.includes(projectId),
    [projectActivityIds, agentChatActivityIds]
  )

  const handleCreateGroup = useCallback((): void => {
    setNewGroupModal({ isOpen: true })
  }, [])

  const handleCreateGroupSubmit = useCallback(
    (name: string) => {
      const newGroupId = addGroup(name)
      if (newGroupModal.projectIdToMove) {
        moveProjectToGroup(newGroupModal.projectIdToMove, newGroupId)
      }
    },
    [addGroup, moveProjectToGroup, newGroupModal.projectIdToMove]
  )

  const handleAddNewProjectToGroup = useCallback(
    async (groupId: string) => {
      try {
        const result = await dialogApi.selectDirectory()
        if (result.success && result.data) {
          const projectPath = result.data
          const folderName = projectPath.split(/[\\/]/).pop() || t('newProject')
          const newProject = addProject(folderName, 'blue', projectPath)
          moveProjectToGroup(newProject.id, groupId)
          toast({
            title: t('projectCreated'),
            description: t('projectCreatedInGroup', { name: folderName })
          })
        }
      } catch (err) {
        console.error('Failed to create project:', err)
        toast({
          title: t('error'),
          description: t('failedCreateFromFolder'),
          variant: 'destructive'
        })
      }
    },
    [addProject, moveProjectToGroup, t]
  )

  const handleGroupContextMenu = useCallback((e: React.MouseEvent): void => {
    // F1: no preventDefault() — Radix's `<ContextMenuTrigger asChild>` composes
    // this handler ahead of its own handleOpen (checkForDefaultPrevented: true);
    // a preventDefault here would make Radix skip opening the menu. Radix's
    // own handleContextMenu already suppresses the native menu.
    e.stopPropagation()
    // Capture the pointer so the `ColorPickerPopover` (opened from the group
    // menu's "Change Color" item) opens near the right-click.
    contextMenuPosRef.current = { x: e.clientX, y: e.clientY }
  }, [])

  const handleStartRenameGroup = useCallback(
    (groupId: string): void => {
      const group = groups.find((g) => g.id === groupId)
      if (group) {
        setEditingGroupId(groupId)
        setEditGroupName(group.name)
      }
    },
    [groups]
  )

  const handleConfirmDeleteGroup = useCallback(
    (groupId: string, deleteProjects: boolean): void => {
      const group = groups.find((g) => g.id === groupId)
      if (group) {
        setGroupDeleteConfirm({
          isOpen: true,
          groupId,
          groupName: group.name,
          deleteProjects
        })
      }
    },
    [groups]
  )

  const handleDeleteGroup = useCallback((): void => {
    if (groupDeleteConfirm.groupId) {
      removeGroup(groupDeleteConfirm.groupId, groupDeleteConfirm.deleteProjects)
    }
    setGroupDeleteConfirm({ isOpen: false, groupId: '', groupName: '', deleteProjects: false })
  }, [groupDeleteConfirm.groupId, groupDeleteConfirm.deleteProjects, removeGroup])

  const renderGroupContextMenu = useCallback(
    (groupId: string): React.ReactNode => {
      const currentGroup = groups.find((g) => g.id === groupId)
      const activeProjects = projects.filter((p) => !p.isArchived)
      return (
        <ContextMenuContent className="w-56">
          <ContextMenuItem onSelect={() => handleStartRenameGroup(groupId)}>
            <Edit2 className="mr-2 h-4 w-4" /> {t('renameGroup')}
          </ContextMenuItem>
          <ContextMenuItem
            onSelect={() =>
              handleOpenColorPicker(
                groupId,
                'group',
                contextMenuPosRef.current.x,
                contextMenuPosRef.current.y
              )
            }
          >
            <Palette className="mr-2 h-4 w-4" /> {t('changeColor')}
          </ContextMenuItem>
          <ContextMenuSub>
            <ContextMenuSubTrigger>
              <Plus className="mr-2 h-4 w-4" /> {t('addProject')}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent className="w-48">
              {activeProjects.map((p) => {
                const isProjectInGroup = currentGroup?.projectIds.includes(p.id) ?? false
                return (
                  <ContextMenuCheckboxItem
                    key={p.id}
                    checked={isProjectInGroup}
                    onCheckedChange={(checked) =>
                      moveProjectToGroup(p.id, checked ? groupId : null)
                    }
                    onSelect={(e) => e.preventDefault()}
                  >
                    {p.name}
                  </ContextMenuCheckboxItem>
                )
              })}
              <ContextMenuSeparator />
              <ContextMenuItem onSelect={() => void handleAddNewProjectToGroup(groupId)}>
                <FolderPlus className="mr-2 h-4 w-4" /> {t('importProject')}
              </ContextMenuItem>
              {onImportFromEditor ? (
                <ContextMenuItem onSelect={() => onImportFromEditor(groupId)}>
                  <FolderInput className="mr-2 h-4 w-4" /> {t('editorImport.title')}
                </ContextMenuItem>
              ) : null}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSeparator />
          <ContextMenuItem onSelect={() => handleConfirmDeleteGroup(groupId, false)}>
            <Trash2 className="mr-2 h-4 w-4" /> {t('deleteGroupKeep')}
          </ContextMenuItem>
          <ContextMenuItem
            variant="destructive"
            onSelect={() => handleConfirmDeleteGroup(groupId, true)}
          >
            <Trash2 className="mr-2 h-4 w-4" /> {t('deleteGroupAll')}
          </ContextMenuItem>
        </ContextMenuContent>
      )
    },
    [
      handleStartRenameGroup,
      handleConfirmDeleteGroup,
      projects,
      groups,
      moveProjectToGroup,
      handleAddNewProjectToGroup,
      handleOpenColorPicker,
      onImportFromEditor,
      t
    ]
  )

  const handleContextMenu = useCallback((e: React.MouseEvent): void => {
    // F1: no preventDefault() — Radix's `<ContextMenuTrigger asChild>` composes
    // this handler ahead of its own handleOpen (checkForDefaultPrevented: true);
    // a preventDefault here would make Radix skip opening the menu. Radix's
    // own handleContextMenu already suppresses the native menu.
    e.stopPropagation()
    // Capture the pointer for the ColorPickerPopover.
    contextMenuPosRef.current = { x: e.clientX, y: e.clientY }
  }, [])

  const handleStartRename = useCallback(
    (projectId: string): void => {
      const project = projects.find((p) => p.id === projectId)
      if (project) {
        setEditingId(projectId)
        setEditName(project.name)
      }
    },
    [projects]
  )

  const handleSaveRename = useCallback(
    (projectId: string): void => {
      if (editName.trim()) {
        onUpdateProject(projectId, { name: editName.trim() })
      }
      setEditingId(null)
      setEditName('')
    },
    [editName, onUpdateProject]
  )

  const handleCancelRename = useCallback((): void => {
    setEditingId(null)
    setEditName('')
  }, [])

  const handleConfirmDelete = useCallback(
    (projectId: string): void => {
      const project = projects.find((p) => p.id === projectId)
      if (project) {
        setDeleteConfirm({
          isOpen: true,
          projectId,
          projectName: project.name
        })
      }
    },
    [projects]
  )

  const handleDelete = useCallback((): void => {
    if (deleteConfirm.projectId) {
      onDeleteProject(deleteConfirm.projectId)
    }
    setDeleteConfirm({ isOpen: false, projectId: '', projectName: '' })
  }, [deleteConfirm.projectId, onDeleteProject])

  const handleCancelDelete = useCallback((): void => {
    setDeleteConfirm({ isOpen: false, projectId: '', projectName: '' })
  }, [])

  const handleOpenSettings = useCallback((projectId: string): void => {
    setSettingsDialog({ isOpen: true, projectId })
  }, [])

  const handleCloseSettings = useCallback((): void => {
    setSettingsDialog({ isOpen: false, projectId: '' })
  }, [])

  // Populate form when dialog opens
  useEffect(() => {
    if (settingsDialog.isOpen && settingsDialog.projectId) {
      const project = projects.find((p) => p.id === settingsDialog.projectId)
      if (project) {
        setSettingsName(project.name)
        setSettingsPath(project.path || '')
        setSettingsShell(project.defaultShell || '')
        setSettingsColor(project.color || 'blue')
      }
    }
  }, [settingsDialog.isOpen, settingsDialog.projectId, projects])

  const handleSaveSettings = useCallback(() => {
    const name = settingsName.trim()
    if (!name || !settingsDialog.projectId) {
      return
    }

    onUpdateProject(settingsDialog.projectId, {
      name,
      path: settingsPath.trim() || undefined,
      defaultShell: settingsShell || undefined,
      color: settingsColor
    })
    handleCloseSettings()
  }, [
    settingsDialog.projectId,
    settingsName,
    settingsPath,
    settingsShell,
    settingsColor,
    onUpdateProject,
    handleCloseSettings
  ])

  const handleBrowsePath = useCallback(async (): Promise<void> => {
    try {
      setSettingsPathLoading(true)
      const result = await dialogApi.selectDirectory()
      if (result.success && result.data) {
        setSettingsPath(result.data)
      }
    } catch (err) {
      console.error('Failed to select directory:', err)
    } finally {
      setSettingsPathLoading(false)
    }
  }, [])

  const renderProjectContextMenu = useCallback(
    (project: Project): React.ReactNode => {
      const isGitRepo = project.isGitRepo ?? false
      const currentGroup = groups.find((g) => g.projectIds.includes(project.id))
      const currentShellPath = availableShells?.available.find((s) => {
        const projectShell = project.defaultShell
        if (!projectShell) return false
        if (projectShell === s.path || projectShell === s.name) return true
        const pathBasename = s.path.split(/[\\/]/).pop()
        return projectShell === pathBasename
      })?.path

      return (
        <ContextMenuContent className="w-56">
          <ContextMenuItem
            onSelect={() => {
              selectProject(project.id)
              navigate('/settings')
            }}
          >
            <Settings className="mr-2 h-4 w-4" /> {t('settings')}
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => handleStartRename(project.id)}>
            <Edit2 className="mr-2 h-4 w-4" /> {t('rename')}
          </ContextMenuItem>
          {/* A project that was never pointed at a folder has no path to copy;
              a disabled entry says that better than one that silently does
              nothing. */}
          <ContextMenuItem
            disabled={!project.path}
            onSelect={() => {
              if (project.path) void clipboardApi.writeText(project.path)
            }}
          >
            <Copy className="mr-2 h-4 w-4" /> {t('copyPath')}
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => handleOpenSettings(project.id)}>
            <Settings className="mr-2 h-4 w-4" /> {t('projectSettings')}
          </ContextMenuItem>
          <ContextMenuItem
            onSelect={() =>
              handleOpenColorPicker(
                project.id,
                'project',
                contextMenuPosRef.current.x,
                contextMenuPosRef.current.y
              )
            }
          >
            <Palette className="mr-2 h-4 w-4" /> {t('changeColor')}
          </ContextMenuItem>

          {availableShells && availableShells.available.length > 0 && (
            <ContextMenuSub>
              <ContextMenuSubTrigger>
                <Terminal className="mr-2 h-4 w-4" /> {t('defaultShell')}
              </ContextMenuSubTrigger>
              <ContextMenuSubContent className="w-48">
                <ContextMenuRadioGroup
                  value={currentShellPath ?? ''}
                  onValueChange={(shellPath: string) =>
                    onUpdateProject(project.id, { defaultShell: shellPath })
                  }
                >
                  {availableShells.available.map((shell) => (
                    <ContextMenuRadioItem key={shell.path} value={shell.path}>
                      {shell.displayName}
                    </ContextMenuRadioItem>
                  ))}
                </ContextMenuRadioGroup>
              </ContextMenuSubContent>
            </ContextMenuSub>
          )}

          <ContextMenuSub>
            <ContextMenuSubTrigger>
              <Folder className="mr-2 h-4 w-4" /> {t('moveToGroup')}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent className="w-48">
              <ContextMenuRadioGroup
                value={currentGroup?.id ?? 'root'}
                onValueChange={(targetGroupId: string) => {
                  if (targetGroupId === 'root') {
                    moveProjectToGroup(project.id, null)
                  } else {
                    moveProjectToGroup(project.id, targetGroupId)
                  }
                }}
              >
                <ContextMenuRadioItem value="root">{t('noGroupRoot')}</ContextMenuRadioItem>
                {groups.map((g) => (
                  <ContextMenuRadioItem key={g.id} value={g.id}>
                    {g.name}
                  </ContextMenuRadioItem>
                ))}
              </ContextMenuRadioGroup>
              <ContextMenuSeparator />
              <ContextMenuItem
                onSelect={() => setNewGroupModal({ isOpen: true, projectIdToMove: project.id })}
              >
                <FolderPlus className="mr-2 h-4 w-4" /> {t('createNewGroup')}
              </ContextMenuItem>
            </ContextMenuSubContent>
          </ContextMenuSub>

          <ContextMenuSeparator />
          <ContextMenuItem
            disabled={!isGitRepo}
            onSelect={() => {
              if (isGitRepo) setNewWorktreeModal({ isOpen: true, projectId: project.id })
            }}
          >
            <GitBranch className="mr-2 h-4 w-4" />{' '}
            {isGitRepo ? t('newWorktree') : t('newWorktreeNoGit')}
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => onArchiveProject(project.id)}>
            <Archive className="mr-2 h-4 w-4" /> {t('archive')}
          </ContextMenuItem>
          <ContextMenuItem variant="destructive" onSelect={() => handleConfirmDelete(project.id)}>
            <Trash2 className="mr-2 h-4 w-4" /> {t('delete')}
          </ContextMenuItem>
        </ContextMenuContent>
      )
    },
    [
      availableShells,
      handleStartRename,
      handleOpenSettings,
      handleOpenColorPicker,
      onUpdateProject,
      onArchiveProject,
      handleConfirmDelete,
      selectProject,
      navigate,
      groups,
      moveProjectToGroup,
      t
    ]
  )

  const renderArchivedProjectContextMenu = useCallback(
    (project: Project): React.ReactNode => {
      return (
        <ContextMenuContent className="w-48">
          <ContextMenuItem onSelect={() => onRestoreProject(project.id)}>
            <RotateCcw className="mr-2 h-4 w-4" /> {t('restore')}
          </ContextMenuItem>
          <ContextMenuItem variant="destructive" onSelect={() => handleConfirmDelete(project.id)}>
            <Trash2 className="mr-2 h-4 w-4" /> {t('delete')}
          </ContextMenuItem>
        </ContextMenuContent>
      )
    },
    [onRestoreProject, handleConfirmDelete, t]
  )

  const colorPickerTarget =
    colorPicker.targetType === 'project'
      ? projects.find((p) => p.id === colorPicker.targetId)
      : groups.find((g) => g.id === colorPicker.targetId)

  // Split active and archived projects
  const activeProjects = useMemo(() => projects.filter((p) => !p.isArchived), [projects])
  const archivedProjects = useMemo(() => projects.filter((p) => p.isArchived), [projects])

  // The search box only renders once the list is long enough to be worth filtering.
  const showSearch = shouldShowProjectSearch(projects.length)

  // Apply the search query to each group. Filtering is gated on `showSearch` so a
  // lingering query can never keep the list filtered after the search box unmounts
  // (e.g. project count drops below the threshold). The unfiltered `activeProjects`
  // is kept for shortcut-index math below.
  const trimmedQuery = showSearch ? searchQuery.trim() : ''
  const isSearching = trimmedQuery.length > 0
  const filteredActiveProjects = useMemo(
    () => filterProjects(activeProjects, { searchQuery: trimmedQuery }),
    [activeProjects, trimmedQuery]
  )
  const filteredArchivedProjects = useMemo(
    () => filterProjects(archivedProjects, { searchQuery: trimmedQuery }),
    [archivedProjects, trimmedQuery]
  )

  // Map each active project id to its position in the UNFILTERED active list.
  // The badge reflects this position (not the filtered render index) so the
  // number a user sees doesn't shift around as they type a search query.
  const activeIndexById = useMemo(() => {
    const map = new Map<string, number>()
    activeProjects.forEach((p, i) => {
      map.set(p.id, i)
    })
    return map
  }, [activeProjects])

  // Group mapping for rendering
  const groupedProjectIds = useMemo(() => {
    const ids = new Set<string>()
    groups.forEach((g) => {
      g.projectIds.forEach((pid) => {
        ids.add(pid)
      })
    })
    return ids
  }, [groups])

  const groupProjectsMap = useMemo(() => {
    return groups.map((g) => {
      const projectsInGroup = g.projectIds
        .map((pid) => filteredActiveProjects.find((p) => p.id === pid))
        .filter((p): p is Project => p !== undefined)
      return {
        group: g,
        projects: projectsInGroup
      }
    })
  }, [groups, filteredActiveProjects])

  const ungroupedActiveProjects = useMemo(() => {
    return filteredActiveProjects.filter((p) => !groupedProjectIds.has(p.id))
  }, [filteredActiveProjects, groupedProjectIds])

  const visibleGroups = useMemo(() => {
    return groupProjectsMap.filter((gp) => gp.projects.length > 0 || !isSearching)
  }, [groupProjectsMap, isSearching])

  // Reset a lingering query if the search box is no longer shown.
  useEffect(() => {
    if (!showSearch && searchQuery) setSearchQuery('')
  }, [showSearch, searchQuery])

  // When the active project CHANGES to one that the current query hides (e.g. a
  // project was just created, or a Ctrl+1..9 shortcut selected a hidden project),
  // clear the search so the now-active project becomes visible instead of silently
  // vanishing. Keyed on a change of `activeProjectId` only — searching for OTHER
  // projects while the active one stays put must NOT wipe the query.
  const prevActiveProjectId = useRef(activeProjectId)
  useEffect(() => {
    const changed = prevActiveProjectId.current !== activeProjectId
    prevActiveProjectId.current = activeProjectId
    if (!changed || !isSearching || !activeProjectId) return
    const visible =
      filteredActiveProjects.some((p) => p.id === activeProjectId) ||
      filteredArchivedProjects.some((p) => p.id === activeProjectId)
    if (!visible) setSearchQuery('')
  }, [activeProjectId, isSearching, filteredActiveProjects, filteredArchivedProjects])
  const hasNoSearchResults =
    isSearching && filteredActiveProjects.length === 0 && filteredArchivedProjects.length === 0

  return (
    <aside className="flex h-full w-full min-w-0 flex-col bg-sidebar">
      {/* Header with inline + button */}
      <div className="flex h-8 items-center justify-between border-b border-sidebar-border/70 px-2.5">
        <span className="label-section text-sidebar-foreground">{t('title')}</span>
        <div className="flex items-center gap-0.5">
          <button
            onClick={handleCreateGroup}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            title={t('newGroupFolder')}
            aria-label={t('newGroupFolder')}
          >
            <FolderPlus size={14} />
          </button>
          {onImportFromEditor ? (
            <button
              onClick={() => onImportFromEditor()}
              className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              title={t('editorImport.title')}
              aria-label={t('editorImport.title')}
              data-testid="header-import-editors"
            >
              <FolderInput size={14} />
            </button>
          ) : null}
          <button
            onClick={onNewProject}
            className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            title={t('newProject')}
            aria-label={t('newProject')}
            data-testid="header-new-project"
          >
            <Plus size={14} />
          </button>
        </div>
      </div>

      {/* Project search — flat style matching the file explorer search */}
      {showSearch && (
        <div className="border-b border-sidebar-border/70 px-2.5 py-1.5">
          <div className="relative">
            <Search
              className="pointer-events-none absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
              aria-hidden="true"
            />
            <input
              ref={searchInputRef}
              type="search"
              placeholder={t('searchProjects')}
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Escape' && searchQuery) {
                  e.preventDefault()
                  e.stopPropagation()
                  setSearchQuery('')
                }
              }}
              className="h-8 w-full rounded-none border-0 bg-transparent pl-7 pr-7 text-xs text-foreground outline-none placeholder:text-muted-foreground/60 focus-visible:ring-1 focus-visible:ring-ring/50 [&::-webkit-search-cancel-button]:hidden"
              aria-label={t('searchProjects')}
              data-testid="project-search-input"
            />
            {searchQuery && (
              <button
                onClick={() => {
                  setSearchQuery('')
                  // Clearing unmounts this button; return focus to the input.
                  searchInputRef.current?.focus()
                }}
                className="absolute right-0 top-1/2 inline-flex size-7 -translate-y-1/2 items-center justify-center rounded-md text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                title={t('clearSearch')}
                aria-label={t('clearSearch')}
                data-testid="project-search-clear"
              >
                <X className="size-3.5" />
              </button>
            )}
          </div>
        </div>
      )}

      {/* Project List */}
      <div className="flex-1 overflow-y-auto py-1" data-group-id="root">
        {projects.length === 0 ? (
          <div className="px-3 py-5" role="status">
            <p className="text-xs font-medium text-foreground">{t('noProjects')}</p>
            <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
              {t('createFirstProject')}
            </p>
          </div>
        ) : hasNoSearchResults ? (
          <div
            className="px-3 py-5"
            data-testid="project-search-empty"
            role="status"
            aria-live="polite"
          >
            <p className="text-xs font-medium text-foreground">{t('noProjectsFound')}</p>
            <p className="mt-1 break-words text-xs leading-relaxed text-muted-foreground">
              {t('nothingMatches', { query: trimmedQuery })}
            </p>
          </div>
        ) : (
          <div data-testid="active-projects-container">
            {/* LayoutGroup keeps Reorder layout measurements in sync when an item's
						    own height changes (e.g. expanding/collapsing a project's chat list via the
						    chevron). Without it, the group caches stale item boxes after a
						    height change and drag-to-reorder stops working. */}
            <LayoutGroup>
              {/* Grouped Projects */}
              <Reorder.Group
                axis="y"
                values={visibleGroups}
                onReorder={(reordered) => {
                  if (isSearching) return
                  reorderGroups(reordered.map((gp) => gp.group.id))
                }}
                className="flex flex-col gap-1"
                data-testid="grouped-projects-container"
              >
                {visibleGroups.map((groupEntry) => {
                  const { group, projects: gpProjects } = groupEntry
                  const isCollapsed = group.isCollapsed
                  const isActiveGroup = activeGroupId === group.id
                  return (
                    <Reorder.Item
                      key={group.id}
                      value={groupEntry}
                      drag={isSearching ? false : 'y'}
                      layout="position"
                      className="list-none"
                    >
                      <div className="flex flex-col">
                        {/* Folder Header */}
                        <ContextMenu>
                          <ContextMenuTrigger asChild>
                            <div
                              onContextMenu={handleGroupContextMenu}
                              role="group"
                              aria-label={group.name}
                              className={cn(
                                'mx-1 flex h-7 w-[calc(100%-0.5rem)] cursor-pointer select-none items-center rounded-sm px-1.5 text-left transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent/50',
                                isActiveGroup &&
                                  'bg-sidebar-accent ring-1 ring-inset ring-primary/35',
                                activeDragOverGroupId === group.id &&
                                  'bg-sidebar-accent/80 ring-1 ring-inset ring-ring/40'
                              )}
                              data-group-id={group.id}
                              data-testid={`project-group-${group.id}`}
                            >
                              <button
                                type="button"
                                onClick={(event) => {
                                  event.stopPropagation()
                                  toggleGroupCollapse(group.id)
                                }}
                                className="mr-0.5 inline-flex size-5 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                                aria-expanded={!isCollapsed}
                                aria-label={t(
                                  isCollapsed ? 'expandGroupAria' : 'collapseGroupAria',
                                  {
                                    name: group.name,
                                    defaultValue: `${isCollapsed ? 'Expand' : 'Collapse'} ${group.name}`
                                  }
                                )}
                                data-testid={`project-group-chevron-${group.id}`}
                              >
                                {isCollapsed ? (
                                  <ChevronRight size={12} className="text-muted-foreground" />
                                ) : (
                                  <ChevronDown size={12} className="text-muted-foreground" />
                                )}
                              </button>
                              {editingGroupId === group.id ? (
                                <div className="flex min-w-0 flex-1 items-center self-stretch">
                                  <span
                                    className={cn(
                                      'mr-1.5 flex-shrink-0 inline-flex items-center',
                                      group.color
                                        ? getColorClasses(group.color).text
                                        : 'text-muted-foreground'
                                    )}
                                  >
                                    {isCollapsed ? <Folder size={13} /> : <FolderOpen size={13} />}
                                  </span>
                                  <input
                                    type="text"
                                    value={editGroupName}
                                    onChange={(e) => setEditGroupName(e.target.value)}
                                    onKeyDown={(e) => {
                                      if (e.key === 'Enter') {
                                        if (editGroupName.trim()) {
                                          renameGroup(group.id, editGroupName.trim())
                                        }
                                        setEditingGroupId(null)
                                      } else if (e.key === 'Escape') {
                                        setEditingGroupId(null)
                                      }
                                      e.stopPropagation()
                                    }}
                                    onBlur={() => {
                                      if (editGroupName.trim()) {
                                        renameGroup(group.id, editGroupName.trim())
                                      }
                                      setEditingGroupId(null)
                                    }}
                                    className="mr-2 min-w-0 flex-1 bg-transparent px-1 text-xs text-foreground outline-none ring-1 ring-inset ring-ring/50"
                                    onClick={(e) => e.stopPropagation()}
                                  />
                                  <span className="px-1.5 text-2xs font-normal text-muted-foreground/70">
                                    {gpProjects.length}
                                  </span>
                                </div>
                              ) : (
                                <button
                                  type="button"
                                  onClick={() => handleSelectGroup(group.id)}
                                  className="flex min-w-0 flex-1 cursor-pointer items-center self-stretch"
                                  aria-pressed={isActiveGroup}
                                  aria-label={group.name}
                                >
                                  <span
                                    className={cn(
                                      'mr-1.5 inline-flex shrink-0 items-center',
                                      group.color
                                        ? getColorClasses(group.color).text
                                        : 'text-muted-foreground'
                                    )}
                                  >
                                    {isCollapsed ? <Folder size={13} /> : <FolderOpen size={13} />}
                                  </span>
                                  <span
                                    className={cn(
                                      'min-w-0 flex-1 truncate text-xs leading-4',
                                      isActiveGroup
                                        ? 'font-medium text-foreground'
                                        : 'text-sidebar-foreground'
                                    )}
                                    title={group.name}
                                  >
                                    {group.name}
                                  </span>
                                  <span className="px-1.5 text-2xs font-normal text-muted-foreground/70">
                                    {gpProjects.length}
                                  </span>
                                </button>
                              )}
                            </div>
                          </ContextMenuTrigger>
                          {renderGroupContextMenu(group.id)}
                        </ContextMenu>

                        {/* Projects in Group */}
                        <CollapseExpandMotion
                          open={(!isCollapsed || isSearching) && gpProjects.length > 0}
                        >
                          <Reorder.Group
                            axis="y"
                            values={gpProjects}
                            onReorder={(reordered) => {
                              if (isSearching) return
                              reorderProjectInGroup(
                                group.id,
                                reordered.map((p) => p.id)
                              )
                            }}
                            className="pl-4 flex flex-col"
                            data-group-container-id={group.id}
                          >
                            {gpProjects.map((project) => {
                              const hasActivity = projectHasActivity(project.id)
                              const shortcutIndex = activeIndexById.get(project.id) ?? -1
                              return (
                                <Reorder.Item
                                  key={project.id}
                                  value={project}
                                  drag={isSearching ? false : 'y'}
                                  layout="position"
                                  className="list-none"
                                  whileDrag={{
                                    opacity: 0.82,
                                    pointerEvents: 'none'
                                  }}
                                  onDrag={(_event, info) => {
                                    const element = document.elementFromPoint(
                                      info.point.x,
                                      info.point.y
                                    )
                                    const container = element?.closest('[data-group-container-id]')
                                    const folderHeader = element?.closest('[data-group-id]')
                                    const groupId =
                                      container?.getAttribute('data-group-container-id') ||
                                      folderHeader?.getAttribute('data-group-id') ||
                                      null
                                    if (groupId !== activeDragOverGroupId) {
                                      setActiveDragOverGroupId(groupId)
                                      activeDragOverGroupIdRef.current = groupId
                                    }
                                  }}
                                  onDragEnd={() => {
                                    const targetGroupId = activeDragOverGroupIdRef.current
                                    if (targetGroupId) {
                                      const nextGroupId =
                                        targetGroupId === 'root' ? null : targetGroupId
                                      const currentGroup = groups.find((g) =>
                                        g.projectIds.includes(project.id)
                                      )
                                      const currentGroupId = currentGroup?.id ?? null
                                      if (nextGroupId !== currentGroupId) {
                                        moveProjectToGroup(project.id, nextGroupId)
                                      }
                                    }
                                    setActiveDragOverGroupId(null)
                                    activeDragOverGroupIdRef.current = null
                                  }}
                                >
                                  <ProjectItem
                                    project={project}
                                    isActive={
                                      activeGroupId === null && project.id === activeProjectId
                                    }
                                    isEditing={editingId === project.id}
                                    editName={editName}
                                    shortcut={
                                      shortcutIndex >= 0 && shortcutIndex < 9
                                        ? `Ctrl+${shortcutIndex + 1}`
                                        : undefined
                                    }
                                    hasActivity={hasActivity}
                                    hasError={projectErrorIds.has(project.id)}
                                    onClick={() => onSelectProject(project.id)}
                                    onTerminalClick={() => onOpenProjectTerminal(project.id)}
                                    onContextMenu={handleContextMenu}
                                    renderContextMenu={renderProjectContextMenu}
                                    onEditNameChange={setEditName}
                                    onSaveRename={() => handleSaveRename(project.id)}
                                    onCancelRename={handleCancelRename}
                                    onSettingsClick={() => {
                                      selectProject(project.id)
                                      navigate('/settings')
                                    }}
                                  />
                                </Reorder.Item>
                              )
                            })}
                          </Reorder.Group>
                        </CollapseExpandMotion>
                      </div>
                    </Reorder.Item>
                  )
                })}
              </Reorder.Group>

              {/* Ungrouped Projects */}
              {ungroupedActiveProjects.length > 0 && (
                <Reorder.Group
                  axis="y"
                  values={ungroupedActiveProjects}
                  onReorder={(reordered) => {
                    if (isSearching) return
                    onReorderProjects(reordered.map((p) => p.id))
                  }}
                  className="flex flex-col mt-1"
                  data-testid="ungrouped-projects-container"
                >
                  {ungroupedActiveProjects.map((project) => {
                    const hasActivity = projectHasActivity(project.id)
                    const shortcutIndex = activeIndexById.get(project.id) ?? -1
                    return (
                      <Reorder.Item
                        key={project.id}
                        value={project}
                        drag={isSearching ? false : 'y'}
                        layout="position"
                        className="list-none"
                        whileDrag={{
                          opacity: 0.82,
                          pointerEvents: 'none'
                        }}
                        onDrag={(_event, info) => {
                          const element = document.elementFromPoint(info.point.x, info.point.y)
                          const container = element?.closest('[data-group-container-id]')
                          const folderHeader = element?.closest('[data-group-id]')
                          const groupId =
                            container?.getAttribute('data-group-container-id') ||
                            folderHeader?.getAttribute('data-group-id') ||
                            null
                          if (groupId !== activeDragOverGroupId) {
                            setActiveDragOverGroupId(groupId)
                            activeDragOverGroupIdRef.current = groupId
                          }
                        }}
                        onDragEnd={() => {
                          const targetGroupId = activeDragOverGroupIdRef.current
                          if (targetGroupId) {
                            const nextGroupId = targetGroupId === 'root' ? null : targetGroupId
                            const currentGroup = groups.find((g) =>
                              g.projectIds.includes(project.id)
                            )
                            const currentGroupId = currentGroup?.id ?? null
                            if (nextGroupId !== currentGroupId) {
                              moveProjectToGroup(project.id, nextGroupId)
                            }
                          }
                          setActiveDragOverGroupId(null)
                          activeDragOverGroupIdRef.current = null
                        }}
                      >
                        <ProjectItem
                          project={project}
                          isActive={activeGroupId === null && project.id === activeProjectId}
                          isEditing={editingId === project.id}
                          editName={editName}
                          shortcut={
                            shortcutIndex >= 0 && shortcutIndex < 9
                              ? `Ctrl+${shortcutIndex + 1}`
                              : undefined
                          }
                          hasActivity={hasActivity}
                          hasError={projectErrorIds.has(project.id)}
                          onClick={() => onSelectProject(project.id)}
                          onTerminalClick={() => onOpenProjectTerminal(project.id)}
                          onContextMenu={handleContextMenu}
                          renderContextMenu={renderProjectContextMenu}
                          onEditNameChange={setEditName}
                          onSaveRename={() => handleSaveRename(project.id)}
                          onCancelRename={handleCancelRename}
                          onSettingsClick={() => {
                            selectProject(project.id)
                            navigate('/settings')
                          }}
                        />
                      </Reorder.Item>
                    )
                  })}
                </Reorder.Group>
              )}
            </LayoutGroup>

            {/* Archived Projects Section */}
            {filteredArchivedProjects.length > 0 && (
              <div className="mt-2">
                <button
                  onClick={() => setShowArchived(!showArchived)}
                  disabled={isSearching}
                  className="label-section mx-1 mt-1 flex h-7 w-[calc(100%-0.5rem)] items-center rounded-sm px-1.5 text-sidebar-foreground transition-colors duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent/50 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-default disabled:hover:bg-transparent"
                  aria-expanded={showArchived || isSearching}
                  aria-label={t('archivedToggleAria')}
                >
                  {showArchived || isSearching ? (
                    <ChevronDown size={14} className="mr-2" />
                  ) : (
                    <ChevronRight size={14} className="mr-2" />
                  )}
                  {t('archived', { count: filteredArchivedProjects.length })}
                </button>
                {(showArchived || isSearching) &&
                  filteredArchivedProjects.map((project) => {
                    const hasActivity = projectHasActivity(project.id)
                    return (
                      <ArchivedProjectItem
                        key={project.id}
                        project={project}
                        hasActivity={hasActivity}
                        hasError={projectErrorIds.has(project.id)}
                        onClick={() => onSelectProject(project.id)}
                        onContextMenu={handleContextMenu}
                        renderContextMenu={renderArchivedProjectContextMenu}
                      />
                    )
                  })}
              </div>
            )}
          </div>
        )}
      </div>

      {/* SSH Connections - Resizable */}
      <SSHResizableSection
        onSSHConnect={onSSHConnect}
        onSelectProfile={onSelectSSHProfile}
        activeProfileId={activeSSHProfileId}
      />

      {/* Version - pinned bottom */}
      <div className="border-t border-sidebar-border/60 px-2 py-1">
        <div className="inline-flex h-6 w-full items-center justify-center">
          <span className="text-3xs text-muted-foreground/60">Termul v0.4.10</span>
        </div>
      </div>

      {/* Group Delete Confirmation Dialog */}
      <ConfirmDialog
        isOpen={groupDeleteConfirm.isOpen}
        title={t('deleteGroupFolder')}
        message={
          groupDeleteConfirm.deleteProjects
            ? t('deleteGroupAllConfirm', { name: groupDeleteConfirm.groupName })
            : t('deleteGroupKeepConfirm', { name: groupDeleteConfirm.groupName })
        }
        confirmLabel={t('delete')}
        cancelLabel={t('cancel')}
        variant="danger"
        onConfirm={handleDeleteGroup}
        onCancel={() =>
          setGroupDeleteConfirm({
            isOpen: false,
            groupId: '',
            groupName: '',
            deleteProjects: false
          })
        }
      />

      {/* Color Picker Popover */}
      {colorPicker.isOpen && colorPickerTarget && (
        <ColorPickerPopover
          x={colorPicker.x}
          y={colorPicker.y}
          currentColor={colorPickerTarget.color || 'blue'}
          onSelectColor={handleColorChange}
          onClose={closeColorPicker}
        />
      )}

      {/* Project Settings Dialog */}
      {settingsDialog.isOpen && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-[2px]"
          onClick={handleCloseSettings}
        >
          <motion.div
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.15 }}
            className="w-[500px] overflow-hidden rounded-md border border-border/80 bg-card shadow-[0_18px_60px_hsl(var(--background)/0.7),inset_0_1px_0_0_hsl(var(--foreground)/0.05)]"
            onClick={(e) => e.stopPropagation()}
          >
            {/* Header */}
            <div className="flex h-9 items-center justify-between border-b border-border/70 px-3">
              <h3 className="text-xs font-semibold tracking-[-0.01em] text-foreground">
                {t('projectSettings')}
              </h3>
              <button
                type="button"
                onClick={handleCloseSettings}
                className="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <X size={14} />
              </button>
            </div>

            {/* Form */}
            <div className="space-y-3 p-4">
              {/* Name Field */}
              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('projectName')}
                </label>
                <input
                  type="text"
                  value={settingsName}
                  onChange={(e) => setSettingsName(e.target.value)}
                  className="h-8 w-full rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                  placeholder={t('projectName')}
                />
              </div>

              {/* Path Field */}
              <div className="space-y-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t('projectPath')}
                </label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={settingsPath}
                    onChange={(e) => setSettingsPath(e.target.value)}
                    className="h-8 flex-1 rounded-md border border-input/80 bg-secondary/35 px-2.5 text-sm text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                    placeholder={t('noDirectorySelected')}
                  />
                  <button
                    type="button"
                    onClick={handleBrowsePath}
                    disabled={settingsPathLoading}
                    className="inline-flex h-8 items-center rounded-md border border-border/80 bg-secondary/50 px-3 text-xs text-foreground transition-colors hover:bg-secondary focus:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-45"
                  >
                    {t('browse')}
                  </button>
                </div>
                <p className="text-xs text-muted-foreground">{t('optionalDefaultDirectory')}</p>
              </div>

              {/* Color Picker */}
              <div className="space-y-2 mt-4">
                <label className="block text-xs font-medium text-muted-foreground mb-1">
                  {t('color')}
                </label>
                <div className="flex gap-2">
                  {availableColors.map((color) => {
                    const colors = getColorClasses(color)
                    return (
                      <button
                        key={color}
                        type="button"
                        onClick={() => setSettingsColor(color)}
                        className={cn(
                          'size-6 rounded-sm transition-[opacity,box-shadow]',
                          colors.bg,
                          settingsColor === color
                            ? 'ring-1 ring-foreground/70 ring-offset-2 ring-offset-card'
                            : 'hover:opacity-80'
                        )}
                      />
                    )
                  })}
                </div>
              </div>

              {/* Shell Field */}
              <div className="space-y-2">
                <label className="block text-xs font-medium text-muted-foreground mb-1">
                  {t('defaultTerminal')}
                </label>
                {availableShells ? (
                  <div className="relative">
                    <select
                      value={settingsShell}
                      onChange={(e) => setSettingsShell(e.target.value)}
                      className="h-8 w-full cursor-pointer appearance-none rounded-md border border-input/80 bg-secondary/35 px-2.5 pr-8 text-sm text-foreground outline-none focus-visible:border-ring/70 focus-visible:ring-1 focus-visible:ring-ring/35"
                    >
                      {availableShells.available.map((shell) => (
                        <option key={shell.path} value={shell.path}>
                          {shell.displayName}
                        </option>
                      ))}
                    </select>
                    <div className="pointer-events-none absolute inset-y-0 right-0 flex items-center px-2 text-muted-foreground">
                      <ChevronDown size={14} />
                    </div>
                  </div>
                ) : (
                  <Skeleton className="w-full h-9 rounded" />
                )}
              </div>
            </div>

            {/* Footer */}
            <div className="flex h-10 items-center justify-end gap-2 border-t border-border/70 bg-secondary/20 px-4">
              <button
                type="button"
                onClick={handleCloseSettings}
                className="inline-flex h-8 items-center rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('cancel')}
              </button>
              <button
                type="button"
                onClick={handleSaveSettings}
                className="inline-flex h-8 items-center rounded-md bg-primary px-3 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 focus:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {t('saveChanges')}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}

      {/* Delete Confirmation Dialog */}
      <ConfirmDialog
        isOpen={deleteConfirm.isOpen}
        title={t('deleteProject')}
        message={t('deleteProjectConfirm', { name: deleteConfirm.projectName })}
        confirmLabel={t('delete')}
        cancelLabel={t('cancel')}
        variant="danger"
        onConfirm={handleDelete}
        onCancel={handleCancelDelete}
      />

      {/* New Worktree Modal */}
      <NewWorktreeModal
        isOpen={newWorktreeModal.isOpen}
        onClose={() => setNewWorktreeModal({ isOpen: false, projectId: '' })}
        projectId={newWorktreeModal.projectId}
      />

      {/* New Group Modal */}
      <NewGroupModal
        isOpen={newGroupModal.isOpen}
        onClose={() => setNewGroupModal({ isOpen: false })}
        onSubmit={handleCreateGroupSubmit}
      />
    </aside>
  )
}

interface ProjectItemProps {
  project: Project
  isActive: boolean
  isEditing: boolean
  editName: string
  shortcut?: string
  hasActivity: boolean
  hasError?: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
  onEditNameChange: (name: string) => void
  onSaveRename: () => void
  onCancelRename: () => void
  onSettingsClick: () => void
  onTerminalClick: () => void
  renderContextMenu?: (project: Project) => React.ReactNode
}

/** The move helpers speak forward slashes; project paths can arrive with backslashes. */
function normalizeProjectPath(path: string): string {
  return path.replace(/\\/g, '/').replace(/\/$/, '')
}

const ProjectItem = memo(function ProjectItem({
  project,
  isActive,
  isEditing,
  editName,
  shortcut,
  hasActivity,
  hasError,
  onClick,
  onContextMenu,
  onEditNameChange,
  onSaveRename,
  onCancelRename,
  onSettingsClick,
  onTerminalClick,
  renderContextMenu
}: ProjectItemProps): React.JSX.Element {
  const { t } = useTranslation('projects')
  const colors = getColorClasses(project.color)
  const inputRef = useRef<HTMLInputElement>(null)
  const dragPaths = useFileExplorerStore((state) => state.dragPaths)
  const endEntryDrag = useFileExplorerStore((state) => state.endEntryDrag)
  const moveEntries = useFileExplorerStore((state) => state.moveEntries)
  const [isEntryDropTarget, setIsEntryDropTarget] = useState(false)

  // Focus input when editing starts
  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus()
      inputRef.current.select()
    }
  }, [isEditing])

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === 'Enter') {
      e.preventDefault()
      onSaveRename()
    } else if (e.key === 'Escape') {
      e.preventDefault()
      onCancelRename()
    }
  }

  // A project row is the drop target for moving files into another project:
  // it is the only place a project's root is addressable while the file tree
  // is still showing a different project. Rows without a path (a project that
  // was never pointed at a folder) have no destination and stay inert.
  const acceptsEntryDrop =
    !isEditing &&
    Boolean(project.path) &&
    dragPaths.some((path) => !isRejectedMove(path, normalizeProjectPath(project.path ?? '')))

  const handleEntryDragOver = (e: React.DragEvent): void => {
    if (!acceptsEntryDrop) return
    e.preventDefault()
    e.dataTransfer.dropEffect = 'move'
    setIsEntryDropTarget(true)
  }

  const handleEntryDrop = (e: React.DragEvent): void => {
    if (!acceptsEntryDrop) return
    e.preventDefault()
    const paths = dragPaths
    endEntryDrag()
    setIsEntryDropTarget(false)
    void moveEntries(paths, normalizeProjectPath(project.path ?? ''))
  }

  return (
    <div data-testid={`project-item-${project.id}`}>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            onClick={isEditing ? undefined : onClick}
            onContextMenu={onContextMenu}
            data-project-path={project.path ? normalizeProjectPath(project.path) : undefined}
            onDragOver={handleEntryDragOver}
            onDragLeave={() => setIsEntryDropTarget(false)}
            onDrop={handleEntryDrop}
            role="button"
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault()
                if (!isEditing) onClick()
              }
            }}
            className={cn(
              'group mx-1 flex h-7 w-[calc(100%-0.5rem)] cursor-pointer select-none items-center rounded-sm px-1.5 text-left transition-colors duration-150 ease-[var(--ease-out)]',
              isEntryDropTarget
                ? 'bg-primary/15 ring-1 ring-inset ring-primary'
                : isActive
                  ? 'bg-sidebar-accent text-foreground ring-1 ring-inset ring-primary/35'
                  : 'text-sidebar-foreground hover:bg-sidebar-accent/50 hover:text-foreground'
            )}
            aria-current={isActive ? 'page' : undefined}
            aria-label={
              isActive
                ? t('activeProjectAria', { name: project.name })
                : t('projectAria', { name: project.name })
            }
          >
            {!isEditing && (
              <span
                aria-hidden="true"
                data-project-color={project.color}
                className={cn(
                  'mr-1.5 size-1.5 shrink-0 rounded-[1px]',
                  colors.bg,
                  isActive ? 'opacity-80' : 'opacity-45'
                )}
              />
            )}
            {isEditing ? (
              <input
                ref={inputRef}
                type="text"
                value={editName}
                onChange={(e) => onEditNameChange(e.target.value)}
                onKeyDown={handleKeyDown}
                onBlur={onSaveRename}
                className="mx-1 min-w-0 flex-1 bg-transparent px-1 text-xs text-foreground outline-none ring-1 ring-inset ring-ring/50"
                onClick={(e) => e.stopPropagation()}
              />
            ) : (
              <span
                className={cn(
                  'min-w-0 flex-1 truncate text-xs leading-4 transition-colors',
                  // flex-1 min-w-0 is required for truncate to clip inside a flex row
                  isActive
                    ? 'text-foreground'
                    : 'text-sidebar-foreground group-hover:text-foreground'
                )}
                title={project.name}
              >
                {project.name}
              </span>
            )}
            {hasError && (
              <span
                className="mr-1 flex animate-pulse items-center text-warning"
                title={t('terminalCrashed')}
              >
                <AlertTriangle size={12} />
              </span>
            )}
            {!isEditing && shortcut && (
              <span
                className={cn(
                  'mr-1 font-mono text-2xs text-muted-foreground/70 transition-opacity',
                  isActive ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'
                )}
              >
                {shortcut}
              </span>
            )}
            {/* Launching a terminal is explicit now: selecting a project shows
                its folder, and this is how you ask for a shell in it. */}
            {!isEditing && (
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  onTerminalClick()
                }}
                className="mr-0.5 inline-flex size-5 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-sidebar-accent hover:text-foreground group-hover:opacity-100 focus:outline-none focus-visible:opacity-100 focus-visible:ring-1 focus-visible:ring-ring"
                title={t('openTerminal')}
                aria-label={t('openTerminalFor', { name: project.name })}
              >
                <Terminal size={12} />
              </button>
            )}
            {!isEditing && (
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  onSettingsClick()
                }}
                className="mr-0.5 inline-flex size-5 shrink-0 items-center justify-center rounded-md text-muted-foreground opacity-0 transition-opacity hover:bg-sidebar-accent hover:text-foreground group-hover:opacity-100 focus:outline-none focus-visible:opacity-100 focus-visible:ring-1 focus-visible:ring-ring"
                title={t('projectSettings')}
                aria-label={t('settingsFor', { name: project.name })}
              >
                <Settings size={12} />
              </button>
            )}
            {!isEditing && hasActivity && (
              <span
                className="mr-1 flex items-center"
                title={t('activity')}
                style={{ isolation: 'isolate' }}
              >
                <MonochromeSpinner
                  pattern="diagonal"
                  cellSize={2}
                  cellGap={1}
                  cellRadius={0.5}
                  label={t('projectActivity')}
                />
              </span>
            )}
          </div>
        </ContextMenuTrigger>
        {renderContextMenu?.(project)}
      </ContextMenu>
    </div>
  )
})

interface ArchivedProjectItemProps {
  hasActivity: boolean
  hasError?: boolean
  project: Project
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
  renderContextMenu?: (project: Project) => React.ReactNode
}

function ArchivedProjectItem({
  project,
  hasActivity,
  hasError,
  onClick,
  onContextMenu,
  renderContextMenu
}: ArchivedProjectItemProps): React.JSX.Element {
  const { t } = useTranslation('projects')
  const colors = getColorClasses(project.color)

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <button
          onClick={onClick}
          onContextMenu={onContextMenu}
          className="group mx-1 flex h-7 w-[calc(100%-0.5rem)] items-center rounded-sm px-1.5 text-left text-muted-foreground opacity-60 transition-[background-color,opacity] duration-150 ease-[var(--ease-out)] hover:bg-sidebar-accent/50 hover:opacity-100"
          aria-label={t('archivedProjectAria', { name: project.name })}
          data-testid={`archived-project-item-${project.id}`}
        >
          <span
            aria-hidden="true"
            data-project-color={project.color}
            className={cn('mr-1.5 size-1.5 shrink-0 rounded-[1px] opacity-40', colors.bg)}
          />
          <span
            className="min-w-0 flex-1 truncate text-xs leading-4 text-muted-foreground group-hover:text-foreground"
            title={project.name}
          >
            {project.name}
          </span>
          {hasActivity && (
            <span
              className="mr-1 flex items-center"
              title={t('activity')}
              style={{ isolation: 'isolate' }}
            >
              <MonochromeSpinner
                pattern="diagonal"
                cellSize={2}
                cellGap={1}
                cellRadius={0.5}
                label={t('projectActivity')}
              />
            </span>
          )}
          {hasError && (
            <span
              className="mr-1 flex animate-pulse items-center text-warning"
              title={t('terminalCrashed')}
            >
              <AlertTriangle size={10} />
            </span>
          )}
          <Archive size={12} className="mr-0.5 text-muted-foreground" />
        </button>
      </ContextMenuTrigger>
      {renderContextMenu?.(project)}
    </ContextMenu>
  )
}

// ============================================================================
// SSH Resizable Section
// ============================================================================

const SSH_HEIGHT_KEY = 'termul-ssh-panel-height'
const SSH_MIN_HEIGHT = 48
const SSH_MAX_HEIGHT = 400
const SSH_DEFAULT_HEIGHT = 140

function SSHResizableSection({
  onSSHConnect,
  onSelectProfile,
  activeProfileId
}: {
  onSSHConnect?: (profileId: string) => void
  onSelectProfile?: (profileId: string) => void
  activeProfileId?: string | null
}): React.JSX.Element | null {
  const { t } = useTranslation('projects')
  const isVisible = useSSHPanelVisible()
  const [height, setHeight] = useState(() => {
    try {
      const saved = localStorage.getItem(SSH_HEIGHT_KEY)
      if (saved) {
        const parsed = parseInt(saved, 10)
        if (parsed >= SSH_MIN_HEIGHT && parsed <= SSH_MAX_HEIGHT) return parsed
      }
    } catch {
      return SSH_DEFAULT_HEIGHT
    }
    return SSH_DEFAULT_HEIGHT
  })

  const isDragging = useRef(false)
  const startY = useRef(0)
  const startHeight = useRef(0)
  const latestHeight = useRef(height)
  // Tracks the document listeners for the in-flight resize so they can be torn
  // down if the component unmounts mid-drag (e.g. SSH panel toggled off).
  const activeDragCleanup = useRef<(() => void) | null>(null)

  useEffect(() => {
    latestHeight.current = height
  }, [height])

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      isDragging.current = true
      startY.current = e.clientY
      startHeight.current = height
      document.body.style.cursor = 'row-resize'
      document.body.style.userSelect = 'none'

      const handleMouseMove = (ev: MouseEvent) => {
        if (!isDragging.current) return
        // Dragging UP = increase height (startY - currentY)
        const delta = startY.current - ev.clientY
        const newHeight = Math.min(
          SSH_MAX_HEIGHT,
          Math.max(SSH_MIN_HEIGHT, startHeight.current + delta)
        )
        setHeight(newHeight)
      }

      const handleMouseUp = () => {
        isDragging.current = false
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
        activeDragCleanup.current = null
        // Persist
        try {
          localStorage.setItem(SSH_HEIGHT_KEY, String(latestHeight.current))
        } catch {
          // Ignore storage errors in restricted environments.
        }
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
      // Expose a teardown for unmount-during-drag cleanup.
      activeDragCleanup.current = () => {
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
      }
    },
    [height]
  )

  // Persist on height change (debounced via ref)
  useEffect(() => {
    try {
      localStorage.setItem(SSH_HEIGHT_KEY, String(height))
    } catch {
      // Ignore storage errors in restricted environments.
    }
  }, [height])

  // Tear down an in-flight resize: remove the document listeners, reset the body
  // styles, and persist the latest height. Stable across renders (refs only).
  const teardownActiveDrag = useCallback(() => {
    if (!activeDragCleanup.current) return
    activeDragCleanup.current()
    activeDragCleanup.current = null
    isDragging.current = false
    document.body.style.cursor = ''
    document.body.style.userSelect = ''
    try {
      localStorage.setItem(SSH_HEIGHT_KEY, String(latestHeight.current))
    } catch {
      // Ignore storage errors in restricted environments.
    }
  }, [])

  // Clean up an in-flight resize when the component unmounts mid-drag.
  useEffect(() => {
    return () => {
      teardownActiveDrag()
    }
  }, [teardownActiveDrag])

  // Also clean up when the panel is hidden: the component returns null but stays
  // mounted, so the unmount effect above does not run on visibility change.
  useEffect(() => {
    if (!isVisible) {
      teardownActiveDrag()
    }
  }, [isVisible, teardownActiveDrag])

  if (!isVisible) return null

  return (
    <div className="flex-shrink-0 flex flex-col" style={{ height: `${height}px` }}>
      {/* Drag handle */}
      <div
        onMouseDown={handleMouseDown}
        className="group flex h-1.5 shrink-0 cursor-row-resize items-center justify-center border-t border-sidebar-border/70 bg-transparent transition-colors hover:bg-ring/20 active:bg-ring/30"
        title={t('dragToResize')}
      >
        <div className="h-px w-8 bg-muted-foreground/0 transition-colors group-hover:bg-muted-foreground/45" />
      </div>
      {/* SSH Panel content */}
      <div className="flex-1 overflow-hidden">
        <SSHPanel
          onConnect={onSSHConnect}
          onSelectProfile={onSelectProfile}
          activeProfileId={activeProfileId}
        />
      </div>
    </div>
  )
}
