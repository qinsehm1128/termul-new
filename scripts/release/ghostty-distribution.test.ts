import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, test } from 'vitest'

const root = join(dirname(fileURLToPath(import.meta.url)), '../..')
const read = (path: string) => readFileSync(join(root, path), 'utf8')

function productBuildJobs(workflow: string): string[] {
  return workflow
    .split(/(?=^ {2}[a-z][a-z0-9_-]+:)/m)
    .filter(
      (job) =>
        job.includes('- name: Build platform artifacts locally') ||
        job.includes('- name: Build se-server (linux-x64 release)')
    )
}

describe('Ghostty production distribution', () => {
  test('production and canary select real native without changing default development', () => {
    const production = JSON.parse(read('src-tauri/tauri.conf.prod.json'))
    const development = JSON.parse(read('src-tauri/tauri.conf.dev.json'))
    const base = JSON.parse(read('src-tauri/tauri.conf.json'))
    expect(production.build.features).toContain('ghostty-vt')
    expect(development.build?.features ?? []).not.toContain('ghostty-vt')
    expect(base.build?.features ?? []).not.toContain('ghostty-vt')
    expect(read('scripts/build-canary.mjs')).toContain('src-tauri/tauri.conf.prod.json')
  })

  test.each([
    'release',
    'nightly'
  ])('%s provisions verified Zig and runs native behavior gates', (name) => {
    const jobs = productBuildJobs(read(`.github/workflows/${name}.yml`))
    expect(jobs).toHaveLength(2)
    for (const job of jobs) {
      const python = job.indexOf('- name: Setup Python')
      const toolchain = job.indexOf('- name: Prepare checksum-pinned Ghostty build toolchain')
      const nativeTest = job.indexOf('- name: Verify real Ghostty native VT')
      const passiveAudit = job.indexOf('- name: Verify passive Ghostty native linkage')
      const productAudit = job.indexOf('- name: Verify production Ghostty passive linkage')
      const build = job.includes('- name: Build platform artifacts locally')
        ? job.indexOf('- name: Build platform artifacts locally')
        : job.indexOf('- name: Build se-server (linux-x64 release)')
      expect(python).toBeGreaterThan(-1)
      expect(toolchain).toBeGreaterThan(python)
      expect(nativeTest).toBeGreaterThan(toolchain)
      expect(passiveAudit).toBeGreaterThan(nativeTest)
      expect(build).toBeGreaterThan(passiveAudit)
      expect(productAudit).toBeGreaterThan(build)
      expect(job).toContain('python3 scripts/release/prepare-ghostty-toolchain.test.py')
      expect(job).toContain('--cache-dir "$RUNNER_TEMP/se-ghostty-toolchain"')
      expect(job).toContain('--github-env "$GITHUB_ENV"')
      expect(job).toContain('-p se-ghostty-vt --features native')
      expect(job).toContain('python3 scripts/release/verify-ghostty-passive-link.test.py')
      expect(job).toContain('--cargo-target-dir src-tauri/target')
      if (job.includes('- name: Build se-server (linux-x64 release)')) {
        expect(job).toContain('--features standalone-server,ghostty-vt')
        expect(job).toContain('TERMUL_SERVER_BUILD_ID: termul-server-')
        expect(job).toContain('touch src/server_main.rs')
      }
    }
  })

  test('self-exec observers stay within the existing signed component/update contract', () => {
    const inputs = JSON.parse(read('scripts/release/component-build-inputs.json'))
    expect(Object.keys(inputs.components).sort()).toEqual([
      'acpCore',
      'guiNative',
      'renderer',
      'terminalCore'
    ])
    // The parser source is external to the Cargo crate directory. Both
    // standalone/native hosts and long-lived Core identity need this input.
    for (const host of ['guiNative', 'acpCore', 'terminalCore']) {
      expect(inputs.components[host]).toContain('src-tauri/vendor/ghostty-vt')
      expect(inputs.components[host]).toContain('src-tauri/tauri.conf.prod.json')
    }
  })
})
