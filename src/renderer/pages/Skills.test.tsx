import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { SkillsStatus } from '@/lib/skills-api'

const loadStatus = vi.fn()
const refreshStatus = vi.fn()
const installSkill = vi.fn()
const projectSkill = vi.fn()
const previewSkill = vi.fn()
const operationStatus = vi.fn()
const onCatalogChanged = vi.fn(() => () => undefined)

vi.mock('@/lib/skills-api', () => ({
  skillsApi: {
    status: (...args: unknown[]) => loadStatus(...args),
    refresh: (...args: unknown[]) => refreshStatus(...args),
    install: (...args: unknown[]) => installSkill(...args),
    project: (...args: unknown[]) => projectSkill(...args),
    preview: (...args: unknown[]) => previewSkill(...args),
    operationStatus: (...args: unknown[]) => operationStatus(...args),
    repair: vi.fn(),
    readSkill: vi.fn().mockResolvedValue({
      body: '# Hello',
      name: 'demo',
      scope: 'global',
      path: '/tmp/SKILL.md',
      description: 'Demo'
    }),
    onCatalogChanged: (...args: unknown[]) => onCatalogChanged(...args)
  }
}))

vi.mock('@/stores/project-store', () => ({
  useProjectStore: (
    selector: (state: {
      projects: Array<{ id: string; name: string; path?: string; isArchived?: boolean }>
      activeProjectId: string
    }) => unknown
  ) =>
    selector({
      projects: [
        { id: 'p1', name: 'Demo Project', path: '/tmp/project' },
        { id: 'p2', name: 'Other Project', path: '/tmp/other' }
      ],
      activeProjectId: 'p1'
    })
}))

import Skills from './Skills'

const catalog: SkillsStatus = {
  catalog: {
    revision: 1,
    diagnostics: [],
    skills: [
      {
        name: 'demo',
        description: 'Demo skill',
        scope: 'global',
        digest: 'abc',
        metadataDigest: 'def',
        status: 'available',
        conflict: false,
        sources: [
          {
            provider: 'agents',
            scope: 'global',
            skillMdPath: '/tmp/demo/SKILL.md',
            digest: 'abc',
            metadataDigest: 'def'
          }
        ]
      }
    ]
  },
  fallbackPolicy: 'ask',
  stale: false
}

describe('Skills page', () => {
  beforeEach(() => {
    loadStatus.mockReset()
    refreshStatus.mockReset()
    installSkill.mockReset()
    projectSkill.mockReset()
    previewSkill.mockReset()
    operationStatus.mockReset()
    refreshStatus.mockResolvedValue(catalog)
    loadStatus.mockResolvedValue(catalog)
    installSkill.mockResolvedValue({
      name: 'demo',
      digest: 'abc',
      canonicalPath: '/tmp',
      projections: []
    })
  })

  it('renders catalog rows and opens a confirmation dialog for install', async () => {
    render(<Skills />)
    expect(await screen.findByText('demo')).toBeInTheDocument()
    fireEvent.click(screen.getByText('demo'))
    expect(await screen.findByText('Canonical')).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Install source path'), {
      target: { value: '/tmp/demo/SKILL.md' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    expect(await screen.findByText('Install this skill?')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(installSkill).toHaveBeenCalled())
  })

  it('asks to overwrite when install reports an unmanaged collision token', async () => {
    installSkill.mockRejectedValueOnce(new Error('UNMANAGED_COLLISION: confirmToken=tok-1'))
    render(<Skills />)
    fireEvent.click(await screen.findByText('demo'))
    fireEvent.change(screen.getByLabelText('Install source path'), {
      target: { value: '/tmp/demo/SKILL.md' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm' }))
    expect(await screen.findByText('Overwrite unmanaged collision?')).toBeInTheDocument()
  })

  it('keeps the confirmation dialog open when projection asks for copy fallback', async () => {
    projectSkill.mockRejectedValueOnce(new Error('PROJECTION_FALLBACK_CONFIRMATION_REQUIRED'))
    render(<Skills />)
    fireEvent.click(await screen.findByText('demo'))
    fireEvent.click(screen.getByRole('button', { name: 'Project' }))
    expect(await screen.findByText('Create provider projection?')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    expect(await screen.findByText('Copy projection instead of symlink?')).toBeInTheDocument()
  })

  it('previews a remote skill into an explicit project instead of the active one', async () => {
    previewSkill.mockResolvedValue({ jobId: 'job-preview' })
    operationStatus.mockResolvedValue({
      jobId: 'job-preview',
      phase: 'preview_ready',
      result: {
        previewId: 'pv-1',
        name: 'remote-demo',
        actualSha256: 'aa',
        mode: 'installAndProject',
        providerIds: [],
        source: { type: 'github', repositoryOrUrl: 'owner/repo' },
        scope: { type: 'project', projectId: 'p2' }
      }
    })
    render(<Skills />)
    expect(await screen.findByText('demo')).toBeInTheDocument()
    fireEvent.change(screen.getByLabelText('Install to'), { target: { value: 'p2' } })
    fireEvent.change(screen.getByPlaceholderText('owner/repository'), {
      target: { value: 'owner/repo' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'Preview source' }))
    await waitFor(() =>
      expect(previewSkill).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: { type: 'project', projectId: 'p2' },
          source: { type: 'github', repositoryOrUrl: 'owner/repo' }
        })
      )
    )
  })

  it('installs a local path into the chosen project, not the selected skill scope', async () => {
    render(<Skills />)
    fireEvent.click(await screen.findByText('demo'))
    fireEvent.change(screen.getByLabelText('Install this skill to'), { target: { value: 'p2' } })
    fireEvent.change(screen.getByLabelText('Install source path'), {
      target: { value: '/tmp/demo/SKILL.md' }
    })
    fireEvent.click(screen.getByRole('button', { name: 'Install' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Confirm' }))
    await waitFor(() =>
      expect(installSkill).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: 'project',
          projectId: 'p2',
          projectRoot: '/tmp/other',
          sourcePath: '/tmp/demo/SKILL.md'
        })
      )
    )
  })

  it('filters by scope from the tablist', async () => {
    render(<Skills />)
    expect(await screen.findByText('demo')).toBeInTheDocument()
    fireEvent.click(screen.getByRole('tab', { name: 'project' }))
    expect(screen.queryByText('demo')).not.toBeInTheDocument()
  })
})
