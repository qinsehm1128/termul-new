import { resolveTerminalScreenEngine } from '@shared/types/terminal-screen.types'
import { describe, expect, it } from 'vitest'
import { hasStructuralProjectChange, toProjectSummaries } from '@/hooks/use-projects-persistence'
import type { Project } from '@/types/project'

const project: Project = { id: 'p1', name: 'Project', color: 'blue', isActive: true }

describe('project terminal screen engine contract', () => {
  it.each([
    undefined,
    null,
    {},
    { terminalScreenEngine: null },
    { terminalScreenEngine: 'future' }
  ])('defaults old or unknown project preferences to existing: %j', (value) =>
    expect(resolveTerminalScreenEngine(value)).toBe('existing'))

  it('resolves only the explicit Ghostty preference without mutating the input', () => {
    const configured = Object.freeze({ terminalScreenEngine: 'ghostty-vt' })
    expect(resolveTerminalScreenEngine(configured)).toBe('ghostty-vt')
    expect(resolveTerminalScreenEngine({ terminalScreenEngine: 'existing' })).toBe('existing')
  })

  it('includes only the enum in project summaries, never environment values', () => {
    const summaries = toProjectSummaries([
      {
        ...project,
        terminalScreenEngine: 'ghostty-vt',
        envVars: [{ key: 'API_KEY', value: 'private-secret', isSecret: true }]
      }
    ])
    expect(summaries[0].terminalScreenEngine).toBe('ghostty-vt')
    expect(JSON.stringify(summaries)).not.toContain('private-secret')
    expect(JSON.stringify(summaries)).not.toContain('API_KEY')
    expect(summaries[0]).not.toHaveProperty('envVars')
  })

  it('persists engine changes but does not treat the implicit default as a structural change', () => {
    expect(
      hasStructuralProjectChange([{ ...project, terminalScreenEngine: 'existing' }], [project])
    ).toBe(false)
    expect(
      hasStructuralProjectChange([{ ...project, terminalScreenEngine: 'ghostty-vt' }], [project])
    ).toBe(true)
  })
})
