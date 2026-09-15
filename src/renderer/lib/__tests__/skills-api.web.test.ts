/**
 * Web-branch tests for skills-api.ts (CAP-2: Web & Mobile 1:1 Parity).
 *
 * `skillsApi.listSkills`/`readSkill` now branch on `isTauriContext()` between
 * the desktop `invoke(...)` path and the same-origin `webServerSkills.*` HTTP
 * impl. This file asserts, one method at a time, that:
 * - the WEB branch calls `fetch` (GET /skills[?projectRoot=] + /skills/:name)
 *   and NOT `invoke`
 * - the DESKTOP branch calls `invoke` and NOT `fetch`
 *
 * Mirrors `git-api.web.test.ts`. Does NOT mock `./web-server-api` so the real
 * `webServerSkills` HTTP impl (fetch + IpcBody unwrap + throw-on-!success) is
 * exercised end-to-end.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const { mockFetch, mockIsTauriContext, mockInvoke } = vi.hoisted(() => ({
  mockFetch: vi.fn(),
  mockIsTauriContext: vi.fn(),
  mockInvoke: vi.fn()
}))

vi.mock('../tauri-runtime', () => ({
  isTauriContext: mockIsTauriContext
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: mockInvoke
}))

vi.mock('../tauri-event', () => ({
  listen: vi.fn(async () => () => undefined)
}))

import { skillsApi } from '../skills-api'

function jsonResponse(body: unknown, status = 200): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    json: () => Promise.resolve(body)
  } as unknown as Response
}

const SAMPLE_SKILL = {
  name: 'forge-idea',
  description: 'd',
  scope: 'global',
  path: '/x/SKILL.md'
}
const SAMPLE_CONTENT = {
  name: 'forge-idea',
  description: 'd',
  scope: 'global',
  body: 'b',
  path: '/x/SKILL.md'
}

describe('skillsApi (web vs desktop branch)', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockFetch.mockReset()
    vi.stubGlobal('fetch', mockFetch)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('web: listSkills fetches GET /skills and unwraps data', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: [SAMPLE_SKILL] }))

    const result = await skillsApi.listSkills()

    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/skills`,
      expect.objectContaining({ method: 'GET' })
    )
    expect(mockInvoke).not.toHaveBeenCalled()
    expect(result).toEqual([SAMPLE_SKILL])
  })

  it('web: listSkills passes projectRoot as a query param', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: [] }))

    await skillsApi.listSkills('/web/proj')

    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/skills?projectRoot=${encodeURIComponent('/web/proj')}`,
      expect.objectContaining({ method: 'GET' })
    )
    expect(mockInvoke).not.toHaveBeenCalled()
  })

  it('web: listSkills throws on !success (server error)', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(
      jsonResponse({ success: false, error: 'scan failed', code: 'SKILLS_SCAN_ERROR' })
    )
    await expect(skillsApi.listSkills()).rejects.toThrow('scan failed')
  })

  it('desktop: listSkills invokes list_agent_skills_cmd with projectRoot', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValueOnce([SAMPLE_SKILL])

    const result = await skillsApi.listSkills('/web/proj')

    expect(mockInvoke).toHaveBeenCalledWith('list_agent_skills_cmd', {
      projectRoot: '/web/proj'
    })
    expect(mockFetch).not.toHaveBeenCalled()
    expect(result).toEqual([SAMPLE_SKILL])
  })

  it('desktop: listSkills passes null projectRoot when omitted', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValueOnce([])

    await skillsApi.listSkills()

    expect(mockInvoke).toHaveBeenCalledWith('list_agent_skills_cmd', { projectRoot: null })
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('web: readSkill fetches GET /skills/:name and unwraps data', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: SAMPLE_CONTENT }))

    const result = await skillsApi.readSkill('forge-idea')

    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/skills/forge-idea`,
      expect.objectContaining({ method: 'GET' })
    )
    expect(mockInvoke).not.toHaveBeenCalled()
    expect(result).toEqual(SAMPLE_CONTENT)
  })

  it('web: readSkill URL-encodes the name + passes projectRoot', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(jsonResponse({ success: true, data: SAMPLE_CONTENT }))

    await skillsApi.readSkill('my skill', '/web/proj')

    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/skills/${encodeURIComponent('my skill')}?projectRoot=${encodeURIComponent('/web/proj')}`,
      expect.objectContaining({ method: 'GET' })
    )
    expect(mockInvoke).not.toHaveBeenCalled()
  })

  it('desktop: readSkill invokes read_agent_skill_cmd', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValueOnce(SAMPLE_CONTENT)

    const result = await skillsApi.readSkill('forge-idea', '/web/proj')

    expect(mockInvoke).toHaveBeenCalledWith('read_agent_skill_cmd', {
      name: 'forge-idea',
      projectRoot: '/web/proj'
    })
    expect(mockFetch).not.toHaveBeenCalled()
    expect(result).toEqual(SAMPLE_CONTENT)
  })

  it('desktop: readSkill passes null projectRoot when omitted', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValueOnce(SAMPLE_CONTENT)

    await skillsApi.readSkill('forge-idea')

    expect(mockInvoke).toHaveBeenCalledWith('read_agent_skill_cmd', {
      name: 'forge-idea',
      projectRoot: null
    })
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('web: status fetches GET /skills/status with projectId', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(
      jsonResponse({
        success: true,
        data: { catalog: { revision: 1, skills: [], diagnostics: [] }, fallbackPolicy: 'ask' }
      })
    )

    await skillsApi.status({ projectId: 'p1' })

    expect(mockFetch).toHaveBeenCalledWith(
      `${window.location.origin}/skills/status?projectId=p1`,
      expect.objectContaining({ method: 'GET' })
    )
    expect(mockInvoke).not.toHaveBeenCalled()
  })

  it('web: status preserves the stable server error code', async () => {
    mockIsTauriContext.mockReturnValue(false)
    mockFetch.mockResolvedValueOnce(
      jsonResponse({
        success: false,
        error: 'project is not registered',
        code: 'PROJECT_NOT_REGISTERED'
      })
    )

    await expect(skillsApi.status({ projectId: 'missing' })).rejects.toThrow(
      'PROJECT_NOT_REGISTERED: project is not registered'
    )
  })

  it('desktop: install invokes skills_install_cmd', async () => {
    mockIsTauriContext.mockReturnValue(true)
    mockInvoke.mockResolvedValueOnce({
      name: 'demo',
      digest: 'abc',
      canonicalPath: '/tmp',
      projections: []
    })

    await skillsApi.install({
      name: 'demo',
      sourcePath: '/tmp/SKILL.md'
    })

    expect(mockInvoke).toHaveBeenCalledWith('skills_install_cmd', {
      request: { name: 'demo', sourcePath: '/tmp/SKILL.md' }
    })
    expect(mockFetch).not.toHaveBeenCalled()
  })
})
