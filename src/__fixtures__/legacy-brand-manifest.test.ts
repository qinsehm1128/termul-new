/**
 * T-H00 — sha256 guard over the frozen TypeScript legacy-brand fixture root.
 *
 * The fixtures encode what a *pre*-rename install left on disk. Every Wave-1
 * harness test reads them from disk rather than inlining a literal, so that a
 * repo-wide `sed 's/termul/se-manager/g'` cannot rewrite the assertion and its
 * subject in one stroke and leave the suite green.
 *
 * This manifest is what makes that structural rather than aspirational: a sed
 * that also rewrote the fixtures breaks every hash below, and a sha256 is a hex
 * constant containing no brand string — the same sed cannot repair it.
 *
 * This file deliberately lives *outside* the frozen root so the root holds only
 * fixture data (FORBID-03: no task may modify a file under the root).
 *
 * If this test goes red: a fixture changed. That is the failure, not the test.
 * Do not run `scripts/gen-legacy-brand-manifest.mjs` to make it green.
 */
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { listFixtureFiles, MANIFEST_NAME } from '../../scripts/gen-legacy-brand-manifest.mjs'

const ROOT = 'src/__fixtures__/legacy-brand'

/** Parse `<sha256>  <path>` lines into `path -> sha256`. */
function readManifest(): Map<string, string> {
  const text = readFileSync(join(process.cwd(), ROOT, MANIFEST_NAME), 'utf8')
  const entries = new Map<string, string>()
  for (const line of text.split('\n')) {
    if (line.trim() === '') continue
    const [hash, path] = line.split('  ')
    entries.set(path, hash)
  }
  return entries
}

describe('legacy-brand fixture manifest (TypeScript root)', () => {
  const manifest = readManifest()
  const onDisk = listFixtureFiles(ROOT) as string[]

  it('covers exactly the files present under the frozen root', () => {
    // Both directions matter: a *deleted* fixture is as much a breach of the
    // freeze as a modified one, and an *added* unhashed fixture is a hole.
    expect(onDisk.sort()).toEqual([...manifest.keys()].sort())
  })

  it.each(onDisk)('%s matches its recorded sha256', (relativePath) => {
    const bytes = readFileSync(join(process.cwd(), ROOT, relativePath))
    expect(createHash('sha256').update(bytes).digest('hex')).toBe(manifest.get(relativePath))
  })

  it('still contains the legacy brand strings the harness depends on', () => {
    // A sed that rewrote the fixtures would break the hashes above, but this
    // asserts the *reason* the freeze exists, so a future well-meaning
    // regeneration of the manifest cannot silently launder a rewritten fixture.
    const conversation = readFileSync(
      join(process.cwd(), ROOT, 'conversation-createdBy-termul.json'),
      'utf8'
    )
    expect(JSON.parse(conversation).createdBy).toBe('termul')

    const transcript = readFileSync(
      join(process.cwd(), ROOT, 'chat-transcript-termul-plan.md'),
      'utf8'
    )
    expect(transcript).toContain('```termul-plan')

    const dump = JSON.parse(
      readFileSync(join(process.cwd(), ROOT, 'localstorage-dump.json'), 'utf8')
    ) as Record<string, string>
    expect(Object.keys(dump).some((key) => key.startsWith('termul-store:'))).toBe(true)
    expect(Object.keys(dump).some((key) => key.startsWith('termul:'))).toBe(true)
  })
})
