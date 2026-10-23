import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, test } from 'vitest'
import { parse } from 'yaml'

const root = join(dirname(fileURLToPath(import.meta.url)), '../..')
type Step = {
  name?: string
  run?: string
  shell?: string
  if?: string
  env?: Record<string, string>
  with?: Record<string, unknown>
}
type Workflow = {
  on: { workflow_dispatch: { inputs: Record<string, { required: boolean; type: string }> } }
  jobs: Record<string, { steps: Step[] }>
}
const workflow = (name: string): Workflow =>
  parse(readFileSync(join(root, `.github/workflows/${name}.yml`), 'utf8'))
const preflight = workflow('ghostty-native-preflight')
const steps = preflight.jobs['windows-native'].steps
// biome-ignore lint/suspicious/noTemplateCurlyInString: GitHub Actions expression, not JavaScript interpolation.
const revisionInput = '${{ inputs.expected_revision }}'

// Execute the workflow shell, but replace Python with a failing helper-test
// command and a succeeding audit. The audit must never mask the first failure.
describe('Ghostty audit CI failure propagation', () => {
  test.each([
    'release',
    'nightly',
    'ghostty-native-preflight'
  ])('%s fails immediately when audit self-tests fail', (name) => {
    const auditSteps = Object.values(workflow(name).jobs)
      .flatMap((job) => job.steps ?? [])
      .filter((step) => step.name === 'Verify passive Ghostty native linkage')
    expect(auditSteps).toHaveLength(name === 'ghostty-native-preflight' ? 1 : 2)
    for (const step of auditSteps) {
      expect(step.shell).toBe('bash')
      expect(step.run).toContain('set -euo pipefail')
      const result = spawnSync(
        'bash',
        [
          '-c',
          `python3() {
  if [[ "$1" == "scripts/release/verify-ghostty-passive-link.test.py" ]]; then return 17; fi
  echo AUDIT_EXECUTED
  return 0
}
${step.run}`
        ],
        { cwd: root, encoding: 'utf8' }
      )
      expect(result.error).toBeUndefined()
      expect(result.status).toBe(17)
      expect(result.stdout).not.toContain('AUDIT_EXECUTED')
    }
  })
})

describe('Ghostty manual preflight revision binding', () => {
  const guard = steps.find((step) => step.name === 'Verify immutable preflight revision')!
  const head = spawnSync('git', ['rev-parse', 'HEAD'], {
    cwd: root,
    encoding: 'utf8'
  }).stdout.trim()
  const runGuard = (expected: string, workflowSha = head) =>
    spawnSync('bash', ['-c', guard.run!], {
      cwd: root,
      encoding: 'utf8',
      env: { ...process.env, EXPECTED_REVISION: expected, GITHUB_SHA: workflowSha }
    })

  test('requires an explicit revision, checks it out and verifies it before native tests', () => {
    expect(preflight.on.workflow_dispatch.inputs.expected_revision).toMatchObject({
      required: true,
      type: 'string'
    })
    expect(steps.find((step) => step.name === 'Checkout')?.with).toMatchObject({
      ref: revisionInput,
      'persist-credentials': false
    })
    expect(guard.shell).toBe('bash')
    expect(guard.env?.EXPECTED_REVISION).toBe(revisionInput)
    expect(steps.indexOf(guard)).toBeLessThan(
      steps.findIndex((step) => step.name === 'Verify real Ghostty native VT')
    )
    expect(head).toMatch(/^[0-9a-f]{40}$/)
    expect(runGuard(head).status).toBe(0)
  })

  test.each([
    '',
    'main',
    'a'.repeat(40)
  ])('rejects invalid or stale expected revision %j', (expected) => {
    const result = runGuard(expected)
    expect(result.status).toBe(1)
    expect(result.stderr).toContain('preflight revision mismatch')
  })

  test('rejects workflow/source revision drift even when checkout matches', () => {
    const result = runGuard(head, 'a'.repeat(40))
    expect(result.status).toBe(1)
    expect(result.stderr).toContain('preflight revision mismatch')
  })

  test('requires the diagnostic archive without collecting installers', () => {
    const upload = steps.find(
      (step) => step.name === 'Upload native archive for audit reproduction'
    )!
    expect(upload.if).toBe('always()')
    expect(upload.with).toMatchObject({
      path: 'src-tauri/target/debug/build/se-ghostty-vt-*/out/native/lib/ghostty-vt-static.lib',
      'if-no-files-found': 'error',
      'retention-days': 3
    })
  })
})
