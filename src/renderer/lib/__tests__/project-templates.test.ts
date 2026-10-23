import { beforeEach, describe, expect, it, vi } from 'vitest'
import { i18n } from '@/i18n'
import { filesystemApi } from '../api'
import { gitApi } from '../git-api'
import { logFrontendError } from '../log-api'
import { BUILT_IN_TEMPLATES, scaffoldProject } from '../project-templates'

vi.mock('../api', () => {
  return {
    filesystemApi: {
      createDirectory: vi.fn(),
      createFile: vi.fn()
    }
  }
})

vi.mock('../git-api', () => ({ gitApi: { addIgnoreRules: vi.fn() } }))
vi.mock('../log-api', () => ({ logFrontendError: vi.fn() }))

describe('Project Templates Scaffolding', () => {
  beforeEach(() => {
    vi.mocked(gitApi.addIgnoreRules).mockReset().mockResolvedValue(undefined)
  })

  it('defines the correct set of built-in templates', () => {
    const ids = BUILT_IN_TEMPLATES.map((t) => t.id)
    expect(ids).toContain('empty')
    expect(ids).toContain('node')
    expect(ids).toContain('rust')
    expect(ids).toContain('react')
    expect(ids).toContain('python')
  })

  it('scaffolds empty template by just creating the directory', async () => {
    const mockCreateDir = vi.mocked(filesystemApi.createDirectory)
    mockCreateDir.mockResolvedValueOnce({ success: true, data: undefined })

    const emptyTemplate = BUILT_IN_TEMPLATES.find((t) => t.id === 'empty')!
    const result = await scaffoldProject('/test/path', 'My Project', emptyTemplate)

    expect(result.success).toBe(true)
    expect(mockCreateDir).toHaveBeenCalledWith('/test/path')
    expect(filesystemApi.createFile).not.toHaveBeenCalled()
    expect(gitApi.addIgnoreRules).not.toHaveBeenCalled()
  })

  it('interpolates project name and creates directories and files for Node.js template', async () => {
    const mockCreateDir = vi.mocked(filesystemApi.createDirectory)
    const mockCreateFile = vi.mocked(filesystemApi.createFile)

    mockCreateDir.mockResolvedValue({ success: true, data: undefined })
    mockCreateFile.mockResolvedValue({ success: true, data: undefined })

    const nodeTemplate = BUILT_IN_TEMPLATES.find((t) => t.id === 'node')!
    const result = await scaffoldProject('/test/path', 'My Node Project', nodeTemplate)

    expect(result.success).toBe(true)
    // Should create base directory + subdirectories
    expect(mockCreateDir).toHaveBeenCalledWith('/test/path')
    expect(mockCreateDir).toHaveBeenCalledWith('/test/path/src')

    // Should create the defined files with interpolated name
    expect(mockCreateFile).toHaveBeenCalledWith(
      '/test/path/package.json',
      expect.stringContaining('"name": "my-node-project"')
    )
    expect(mockCreateFile).toHaveBeenCalledWith('/test/path/src/index.js', expect.any(String))
    expect(mockCreateFile).not.toHaveBeenCalledWith('/test/path/.gitignore', expect.any(String))
    expect(gitApi.addIgnoreRules).toHaveBeenCalledWith(
      '/test/path',
      nodeTemplate.files!.find((file) => file.path === '.gitignore')!.content
    )
  })

  it('uses safe host merging for every built-in template ignore file', async () => {
    vi.mocked(filesystemApi.createDirectory).mockResolvedValue({ success: true, data: undefined })
    vi.mocked(filesystemApi.createFile).mockResolvedValue({ success: true, data: undefined })
    for (const template of BUILT_IN_TEMPLATES) {
      if (!template.files?.some((file) => file.path === '.gitignore')) continue
      const result = await scaffoldProject(`/test/${template.id}`, 'test', template)
      expect(result.success).toBe(true)
      expect(gitApi.addIgnoreRules).toHaveBeenCalledWith(`/test/${template.id}`, expect.any(String))
      expect(filesystemApi.createFile).not.toHaveBeenCalledWith(
        `/test/${template.id}/.gitignore`,
        expect.any(String)
      )
    }
  })

  it('reports ignore merge failure without falling back to a truncating write', async () => {
    vi.mocked(filesystemApi.createDirectory).mockResolvedValue({ success: true, data: undefined })
    vi.mocked(filesystemApi.createFile).mockResolvedValue({ success: true, data: undefined })
    vi.mocked(gitApi.addIgnoreRules).mockRejectedValueOnce(new Error('unsafe ignore file'))
    const template = {
      id: 'test',
      name: 'test',
      description: 'test',
      files: [{ path: '.gitignore', content: 'private-pattern-do-not-log\n' }]
    }
    const result = await scaffoldProject('/test/path', 'test', template)
    expect(result).toMatchObject({ success: false, code: 'SCAFFOLD_FILE_ERROR' })
    expect(filesystemApi.createFile).not.toHaveBeenCalledWith(
      '/test/path/.gitignore',
      expect.any(String)
    )
    expect(logFrontendError).toHaveBeenCalledWith(
      expect.objectContaining({ source: 'project-scaffold.gitignore' })
    )
    expect(JSON.stringify(vi.mocked(logFrontendError).mock.calls)).not.toContain(
      'private-pattern-do-not-log'
    )
  })

  it('returns failure if directory creation fails', async () => {
    const mockCreateDir = vi.mocked(filesystemApi.createDirectory)
    mockCreateDir.mockResolvedValueOnce({
      success: false,
      error: 'Permission denied',
      code: 'PERMISSION_ERROR'
    })

    const nodeTemplate = BUILT_IN_TEMPLATES.find((t) => t.id === 'node')!
    const result = await scaffoldProject('/test/path', 'test', nodeTemplate)

    expect(result.success).toBe(false)
    if (!result.success) {
      expect(result.error).toContain('Failed to create base directory')
    } else {
      expect.fail('Expected scaffolding to fail')
    }
  })

  it('localizes scaffold failure framing while preserving backend details', async () => {
    const previousLanguage = i18n.language
    await i18n.changeLanguage('zh-CN')
    try {
      vi.mocked(filesystemApi.createDirectory).mockResolvedValueOnce({
        success: false,
        error: 'Permission denied',
        code: 'PERMISSION_ERROR'
      })

      const nodeTemplate = BUILT_IN_TEMPLATES.find((t) => t.id === 'node')!
      const result = await scaffoldProject('/test/path', 'test', nodeTemplate)

      expect(result).toMatchObject({
        success: false,
        error: '创建基础目录失败：Permission denied',
        code: 'SCAFFOLD_BASE_DIR_ERROR'
      })
    } finally {
      await i18n.changeLanguage(previousLanguage)
    }
  })

  it('preserves leading double slash for UNC paths', async () => {
    const mockCreateDir = vi.mocked(filesystemApi.createDirectory)
    const mockCreateFile = vi.mocked(filesystemApi.createFile)

    mockCreateDir.mockResolvedValue({ success: true, data: undefined })
    mockCreateFile.mockResolvedValue({ success: true, data: undefined })

    const nodeTemplate = BUILT_IN_TEMPLATES.find((t) => t.id === 'node')!
    const result = await scaffoldProject('//server/share/path', 'UNC Project', nodeTemplate)

    expect(result.success).toBe(true)
    expect(mockCreateDir).toHaveBeenCalledWith('//server/share/path')
    expect(mockCreateDir).toHaveBeenCalledWith('//server/share/path/src')
    expect(mockCreateFile).toHaveBeenCalledWith(
      '//server/share/path/package.json',
      expect.any(String)
    )
    expect(gitApi.addIgnoreRules).toHaveBeenCalledWith('//server/share/path', expect.any(String))
  })
})
