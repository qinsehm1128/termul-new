import { describe, expect, it } from 'vitest'
import { cloneTargetPath, parseGitRepoUrl } from './git-clone-url'

describe('parseGitRepoUrl', () => {
  it.each([
    [
      'https://github.com/qinsehm1128/prefect_test.git',
      'https://github.com/qinsehm1128/prefect_test.git'
    ],
    [
      'https://github.com/qinsehm1128/prefect_test',
      'https://github.com/qinsehm1128/prefect_test.git'
    ],
    [
      'https://github.com/qinsehm1128/prefect_test/tree/main/src',
      'https://github.com/qinsehm1128/prefect_test.git'
    ],
    ['github.com/qinsehm1128/prefect_test/', 'https://github.com/qinsehm1128/prefect_test.git'],
    ['qinsehm1128/prefect_test', 'https://github.com/qinsehm1128/prefect_test.git'],
    ['git@github.com:qinsehm1128/prefect_test.git', 'git@github.com:qinsehm1128/prefect_test.git'],
    [
      'ssh://git@gitlab.com/group/sub/prefect_test.git',
      'ssh://git@gitlab.com/group/sub/prefect_test.git'
    ]
  ])('reads %s', (input, cloneUrl) => {
    expect(parseGitRepoUrl(input)).toEqual({ cloneUrl, repoName: 'prefect_test' })
  })

  it.each([
    '',
    'prefect_test',
    'https://github.com/only-owner',
    'file:///etc/passwd',
    '--upload-pack=x/y',
    'https://github.com/a/b c',
    'https://github.com/a/..'
  ])('rejects %j', (input) => {
    expect(parseGitRepoUrl(input)).toBeNull()
  })
})

describe('cloneTargetPath', () => {
  it('joins the parent folder and repository name once', () => {
    expect(cloneTargetPath('/Users/qs/project/ns/python/', 'prefect_test')).toBe(
      '/Users/qs/project/ns/python/prefect_test'
    )
  })
})
