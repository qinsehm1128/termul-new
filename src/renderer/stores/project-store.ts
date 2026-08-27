import { create } from 'zustand'
import { useShallow } from 'zustand/shallow'
import { randomUUID } from '@/lib/uuid'
import type { EnvVariable, Project, ProjectColor, ProjectGroup, Worktree } from '@/types/project'

function isSelectableProject(project: Project | undefined): project is Project {
  return project !== undefined && project.isArchived !== true
}

function getGroupProjectId(group: ProjectGroup, projects: Project[]): string {
  const projectsById = new Map(projects.map((project) => [project.id, project]))
  const preferredProject = group.preferredProjectId
    ? projectsById.get(group.preferredProjectId)
    : undefined

  if (isSelectableProject(preferredProject) && group.projectIds.includes(preferredProject.id)) {
    return preferredProject.id
  }

  return (
    group.projectIds.find((projectId) => isSelectableProject(projectsById.get(projectId))) ?? ''
  )
}

function repairGroup(group: ProjectGroup, projects: Project[]): ProjectGroup {
  const existingProjectIds = new Set(projects.map((project) => project.id))
  const projectIds = group.projectIds.filter((projectId) => existingProjectIds.has(projectId))
  const candidate = getGroupProjectId({ ...group, projectIds }, projects)

  return {
    ...group,
    projectIds,
    preferredProjectId: candidate || undefined
  }
}

function markActiveProject(projects: Project[], activeProjectId: string): Project[] {
  return projects.map((project) => ({
    ...project,
    isActive: project.id === activeProjectId
  }))
}

export interface ProjectState {
  // State
  projects: Project[]
  groups: ProjectGroup[]
  activeProjectId: string
  activeGroupId: string | null
  isLoaded: boolean
  isWorktreeOperationLocked: boolean

  // Actions
  selectProject: (id: string) => void
  selectGroup: (id: string) => void
  addProject: (
    name: string,
    color: ProjectColor,
    path?: string,
    defaultShell?: string,
    envVars?: EnvVariable[]
  ) => Project
  updateProject: (id: string, updates: Partial<Project>) => void
  deleteProject: (id: string) => void
  archiveProject: (id: string) => void
  restoreProject: (id: string) => void
  reorderProjects: (activeProjectIds: string[]) => void
  setProjects: (
    projects: Project[],
    activeProjectId?: string,
    groups?: ProjectGroup[],
    activeGroupId?: string | null
  ) => void
  addWorktree: (projectId: string, worktree: Worktree) => void
  removeWorktree: (projectId: string, worktreeId: string) => void
  setActiveWorktree: (projectId: string, worktreeId: string | null) => void
  setWorktreeOperationLock: (locked: boolean) => void

  // Group Actions
  addGroup: (name: string) => string
  removeGroup: (id: string, deleteProjects: boolean) => void
  renameGroup: (id: string, newName: string) => void
  toggleGroupCollapse: (id: string) => void
  moveProjectToGroup: (projectId: string, targetGroupId: string | null, index?: number) => void
  reorderGroups: (groupIds: string[]) => void
  reorderProjectInGroup: (groupId: string, projectIds: string[]) => void
  updateGroup: (id: string, updates: Partial<Omit<ProjectGroup, 'id'>>) => void
}

export const useProjectStore = create<ProjectState>((set, get) => ({
  projects: [],
  groups: [],
  activeProjectId: '',
  activeGroupId: null,
  isLoaded: false,
  isWorktreeOperationLocked: false,
  setProjects: (
    projects: Project[],
    activeProjectId?: string,
    groups?: ProjectGroup[],
    activeGroupId?: string | null
  ): void => {
    const repairedGroups = (groups ?? []).map((group) => repairGroup(group, projects))
    const activeGroup = repairedGroups.find((group) => group.id === activeGroupId)
    const fallbackProjectId =
      projects.find((project) => project.isArchived !== true)?.id ?? projects[0]?.id ?? ''
    const nextActiveProjectId = activeGroup
      ? getGroupProjectId(activeGroup, projects)
      : projects.some((project) => project.id === activeProjectId)
        ? (activeProjectId ?? '')
        : fallbackProjectId

    set({
      projects: markActiveProject(projects, nextActiveProjectId),
      groups: repairedGroups,
      activeProjectId: nextActiveProjectId,
      activeGroupId: activeGroup?.id ?? null,
      isLoaded: true
    })
  },

  selectProject: (id: string): void => {
    set((state) => {
      const selectedProject = state.projects.find((project) => project.id === id)
      if (!selectedProject) return state

      return {
        activeProjectId: id,
        activeGroupId: null,
        projects: markActiveProject(state.projects, id),
        groups: state.groups.map((group) =>
          group.projectIds.includes(id) && isSelectableProject(selectedProject)
            ? { ...group, preferredProjectId: id }
            : group
        )
      }
    })
  },

  selectGroup: (id: string): void => {
    set((state) => {
      const group = state.groups.find((candidate) => candidate.id === id)
      if (!group) return state

      const activeProjectId = getGroupProjectId(group, state.projects)
      return {
        activeGroupId: id,
        activeProjectId,
        projects: markActiveProject(state.projects, activeProjectId),
        groups: state.groups.map((candidate) =>
          candidate.id === id
            ? { ...candidate, preferredProjectId: activeProjectId || undefined }
            : candidate
        )
      }
    })
  },

  addProject: (
    name: string,
    color: ProjectColor,
    path?: string,
    defaultShell?: string,
    envVars?: EnvVariable[]
  ): Project => {
    const newProject: Project = {
      id: randomUUID(),
      name,
      color,
      path,
      defaultShell,
      envVars,
      gitBranch: 'main'
    }
    set((state) => ({
      projects: markActiveProject([...state.projects, newProject], newProject.id),
      activeProjectId: newProject.id,
      activeGroupId: null
    }))
    return newProject
  },

  updateProject: (id: string, updates: Partial<Project>): void => {
    set((state) => ({
      projects: state.projects.map((p) => (p.id === id ? { ...p, ...updates } : p))
    }))
  },

  deleteProject: (id: string): void => {
    set((state) => {
      const remaining = state.projects.filter((project) => project.id !== id)
      const groups = state.groups.map((group) =>
        repairGroup(
          {
            ...group,
            projectIds: group.projectIds.filter((projectId) => projectId !== id)
          },
          remaining
        )
      )
      const activeGroup = groups.find((group) => group.id === state.activeGroupId)
      const activeProjectId = activeGroup
        ? getGroupProjectId(activeGroup, remaining)
        : state.activeProjectId === id
          ? (remaining.find((project) => project.isArchived !== true)?.id ?? '')
          : state.activeProjectId

      return {
        projects: markActiveProject(remaining, activeProjectId),
        groups,
        activeProjectId,
        activeGroupId: activeGroup?.id ?? null
      }
    })
  },

  archiveProject: (id: string): void => {
    set((state) => {
      const projects = state.projects.map((project) =>
        project.id === id ? { ...project, isArchived: true } : project
      )
      const groups = state.groups.map((group) => repairGroup(group, projects))
      const activeGroup = groups.find((group) => group.id === state.activeGroupId)
      const activeProjectId = activeGroup
        ? getGroupProjectId(activeGroup, projects)
        : state.activeProjectId

      return {
        projects: markActiveProject(projects, activeProjectId),
        groups,
        activeProjectId
      }
    })
  },

  restoreProject: (id: string): void => {
    set((state) => {
      const projects = state.projects.map((project) =>
        project.id === id ? { ...project, isArchived: false } : project
      )
      const groups = state.groups.map((group) => repairGroup(group, projects))
      const activeGroup = groups.find((group) => group.id === state.activeGroupId)
      const activeProjectId = activeGroup
        ? getGroupProjectId(activeGroup, projects)
        : state.activeProjectId

      return {
        projects: markActiveProject(projects, activeProjectId),
        groups,
        activeProjectId
      }
    })
  },

  reorderProjects: (activeProjectIds: string[]): void => {
    set((state) => {
      // Separate archived and active projects
      const archivedProjects = state.projects.filter((p) => p.isArchived)
      const activeProjects = state.projects.filter((p) => !p.isArchived)

      // Create a map for quick lookup
      const projectMap = new Map(activeProjects.map((p) => [p.id, p]))

      // Reorder active projects based on the new order
      const reorderedActive = activeProjectIds
        .map((id) => projectMap.get(id))
        .filter((p): p is Project => p !== undefined)

      // Preserve active projects that were not in the reordered list (e.g. grouped projects)
      const reorderedIdsSet = new Set(activeProjectIds)
      const remainingActive = activeProjects.filter((p) => !reorderedIdsSet.has(p.id))

      // Combine reordered active projects, remaining active projects, and archived projects
      return { projects: [...reorderedActive, ...remainingActive, ...archivedProjects] }
    })
  },

  addWorktree: (projectId: string, worktree: Worktree): void => {
    set((state) => ({
      projects: state.projects.map((p) =>
        p.id === projectId ? { ...p, worktrees: [...(p.worktrees ?? []), worktree] } : p
      )
    }))
  },

  removeWorktree: (projectId: string, worktreeId: string): void => {
    set((state) => ({
      projects: state.projects.map((p) =>
        p.id === projectId
          ? {
              ...p,
              worktrees: (p.worktrees ?? []).filter((w) => w.id !== worktreeId),
              activeWorktreeId: p.activeWorktreeId === worktreeId ? null : p.activeWorktreeId
            }
          : p
      )
    }))
  },

  setActiveWorktree: (projectId: string, worktreeId: string | null): void => {
    set((state) => ({
      projects: state.projects.map((p) =>
        p.id === projectId ? { ...p, activeWorktreeId: worktreeId } : p
      )
    }))
  },

  setWorktreeOperationLock: (locked: boolean): void => {
    set({ isWorktreeOperationLocked: locked })
  },

  // Group Actions Implementation
  addGroup: (name: string): string => {
    const id = randomUUID()
    const newGroup: ProjectGroup = {
      id,
      name,
      projectIds: [],
      isCollapsed: false
    }
    set((state) => ({
      groups: [...state.groups, newGroup]
    }))
    return id
  },

  removeGroup: (id: string, deleteProjects: boolean): void => {
    const { groups, projects, activeProjectId, activeGroupId } = get()
    const groupToRemove = groups.find((g) => g.id === id)
    if (!groupToRemove) return

    let updatedProjects = projects
    if (deleteProjects) {
      updatedProjects = projects.filter((p) => !groupToRemove.projectIds.includes(p.id))
    }

    const nextActiveProjectId = updatedProjects.some((project) => project.id === activeProjectId)
      ? activeProjectId
      : (updatedProjects.find((project) => project.isArchived !== true)?.id ?? '')
    const updatedGroups = groups
      .filter((group) => group.id !== id)
      .map((group) => repairGroup(group, updatedProjects))
    const nextActiveGroup =
      activeGroupId === id ? undefined : updatedGroups.find((group) => group.id === activeGroupId)
    const repairedActiveProjectId = nextActiveGroup
      ? getGroupProjectId(nextActiveGroup, updatedProjects)
      : nextActiveProjectId

    set({
      groups: updatedGroups,
      projects: markActiveProject(updatedProjects, repairedActiveProjectId),
      activeProjectId: repairedActiveProjectId,
      activeGroupId: nextActiveGroup?.id ?? null
    })
  },

  renameGroup: (id: string, newName: string): void => {
    set((state) => ({
      groups: state.groups.map((g) => (g.id === id ? { ...g, name: newName } : g))
    }))
  },

  toggleGroupCollapse: (id: string): void => {
    set((state) => ({
      groups: state.groups.map((g) => (g.id === id ? { ...g, isCollapsed: !g.isCollapsed } : g))
    }))
  },

  moveProjectToGroup: (projectId: string, targetGroupId: string | null, index?: number): void => {
    set((state) => {
      // 1. Remove from all existing groups
      const cleanedGroups = state.groups.map((group) => ({
        ...group,
        projectIds: group.projectIds.filter((id) => id !== projectId)
      }))

      // 2. Add to target group if it exists
      const movedGroups =
        targetGroupId === null
          ? cleanedGroups
          : cleanedGroups.map((group) => {
              if (group.id !== targetGroupId) return group

              const newProjectIds = [...group.projectIds]
              if (typeof index === 'number') {
                newProjectIds.splice(index, 0, projectId)
              } else {
                newProjectIds.push(projectId)
              }
              return { ...group, projectIds: newProjectIds, isCollapsed: false }
            })
      const groups = movedGroups.map((group) => repairGroup(group, state.projects))
      const activeGroup = groups.find((group) => group.id === state.activeGroupId)
      const activeProjectStillInGroup =
        activeGroup?.projectIds.includes(state.activeProjectId) &&
        isSelectableProject(state.projects.find((project) => project.id === state.activeProjectId))
      const activeProjectId = activeGroup
        ? activeProjectStillInGroup
          ? state.activeProjectId
          : getGroupProjectId(activeGroup, state.projects)
        : state.activeProjectId
      const normalizedGroups = activeGroup
        ? groups.map((group) =>
            group.id === activeGroup.id
              ? { ...group, preferredProjectId: activeProjectId || undefined }
              : group
          )
        : groups

      return {
        groups: normalizedGroups,
        activeProjectId,
        projects: markActiveProject(state.projects, activeProjectId)
      }
    })
  },

  reorderGroups: (groupIds: string[]): void => {
    set((state) => {
      const groupMap = new Map(state.groups.map((g) => [g.id, g]))
      const reorderedGroups = groupIds
        .map((id) => groupMap.get(id))
        .filter((g): g is ProjectGroup => g !== undefined)

      // Preserve groups that were not in the reordered list
      const reorderedIdsSet = new Set(groupIds)
      const remainingGroups = state.groups.filter((g) => !reorderedIdsSet.has(g.id))

      return { groups: [...reorderedGroups, ...remainingGroups] }
    })
  },

  reorderProjectInGroup: (groupId: string, projectIds: string[]): void => {
    set((state) => {
      const groups = state.groups.map((group) =>
        group.id === groupId ? repairGroup({ ...group, projectIds }, state.projects) : group
      )
      const activeGroup = groups.find((group) => group.id === state.activeGroupId)
      const activeProjectId = activeGroup
        ? getGroupProjectId(activeGroup, state.projects)
        : state.activeProjectId

      return {
        groups,
        activeProjectId,
        projects: markActiveProject(state.projects, activeProjectId)
      }
    })
  },

  updateGroup: (id: string, updates: Partial<Omit<ProjectGroup, 'id'>>): void => {
    set((state) => {
      const groups = state.groups.map((group) =>
        group.id === id ? repairGroup({ ...group, ...updates }, state.projects) : group
      )
      const activeGroup = groups.find((group) => group.id === state.activeGroupId)
      const activeProjectId = activeGroup
        ? getGroupProjectId(activeGroup, state.projects)
        : state.activeProjectId

      return {
        groups,
        activeProjectId,
        projects: markActiveProject(state.projects, activeProjectId)
      }
    })
  }
}))

// Selectors for performance (selective subscriptions)
export function useActiveProject(): Project | undefined {
  return useProjectStore((state) => state.projects.find((p) => p.id === state.activeProjectId))
}

export function useProjects(): Project[] {
  return useProjectStore(useShallow((state) => state.projects))
}

export function useActiveProjectId(): string {
  return useProjectStore((state) => state.activeProjectId)
}

export function useActiveGroupId(): string | null {
  return useProjectStore((state) => state.activeGroupId)
}

export function useProjectsLoaded(): boolean {
  return useProjectStore((state) => state.isLoaded)
}

export function getActiveWorktreeFromStore(projectId: string): Worktree | undefined {
  const project = useProjectStore.getState().projects.find((p) => p.id === projectId)
  if (!project?.activeWorktreeId) return undefined
  return project.worktrees?.find((w) => w.id === project.activeWorktreeId)
}

export function useProjectActions(): Pick<
  ProjectState,
  | 'selectProject'
  | 'selectGroup'
  | 'addProject'
  | 'updateProject'
  | 'deleteProject'
  | 'archiveProject'
  | 'restoreProject'
  | 'reorderProjects'
  | 'addWorktree'
  | 'removeWorktree'
  | 'setActiveWorktree'
  | 'setWorktreeOperationLock'
  | 'addGroup'
  | 'removeGroup'
  | 'renameGroup'
  | 'toggleGroupCollapse'
  | 'moveProjectToGroup'
  | 'reorderGroups'
  | 'reorderProjectInGroup'
  | 'updateGroup'
> {
  return useProjectStore(
    useShallow((state) => ({
      selectProject: state.selectProject,
      selectGroup: state.selectGroup,
      addProject: state.addProject,
      updateProject: state.updateProject,
      deleteProject: state.deleteProject,
      archiveProject: state.archiveProject,
      restoreProject: state.restoreProject,
      reorderProjects: state.reorderProjects,
      addWorktree: state.addWorktree,
      removeWorktree: state.removeWorktree,
      setActiveWorktree: state.setActiveWorktree,
      setWorktreeOperationLock: state.setWorktreeOperationLock,
      addGroup: state.addGroup,
      removeGroup: state.removeGroup,
      renameGroup: state.renameGroup,
      toggleGroupCollapse: state.toggleGroupCollapse,
      moveProjectToGroup: state.moveProjectToGroup,
      reorderGroups: state.reorderGroups,
      reorderProjectInGroup: state.reorderProjectInGroup,
      updateGroup: state.updateGroup
    }))
  )
}
