import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { notifyAgentSkillsChanged } from '@/lib/agent-skills-events'
import { skillsApi } from '@/lib/skills-api'
import { useAgentSkills } from './use-agent-skills'

describe('useAgentSkills', () => {
  beforeEach(() => {
    vi.restoreAllMocks()
  })

  it('reloads when skills change in its Conversation workspace', async () => {
    const list = vi.spyOn(skillsApi, 'listSkills').mockResolvedValueOnce([])
    const { result } = renderHook(() => useAgentSkills('/conversation/workspace'))

    await waitFor(() => expect(list).toHaveBeenCalledTimes(1))
    list.mockResolvedValueOnce([
      {
        name: 'se-manager-scheduled-tasks',
        description: 'Create project-managed scheduled tasks.',
        scope: 'project',
        path: '/conversation/workspace/.agents/skills/se-manager-scheduled-tasks/SKILL.md'
      }
    ])

    act(() => notifyAgentSkillsChanged('/another/workspace'))
    expect(list).toHaveBeenCalledTimes(1)

    act(() => notifyAgentSkillsChanged('/conversation/workspace'))
    await waitFor(() => expect(result.current.skills).toHaveLength(1))
    expect(list).toHaveBeenNthCalledWith(2, '/conversation/workspace')
  })
})
