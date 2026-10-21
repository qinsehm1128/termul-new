/**
 * Regression check for WebGL glyph corruption after a hidden tab is shown
 * again (see harness.ts). macOS only: it renders in a real WKWebView, the
 * engine Se ships, because jsdom has no WebGL.
 *
 *   bun scripts/webgl-atlas-repro/check.ts
 *
 * Exits non-zero when any policy corrupts, when a stress run never merges atlas
 * pages, or when the detector control sees no difference — a run that never
 * merges, or a blind detector, means the check proves nothing.
 *
 * The legacy policy and the sibling clearTextureAtlas used to be positive
 * controls that corrupted. addon-webgl 0.20.0-beta.288 (xterm.js #6042) and
 * 0.20.0-beta.291 (#6055) fixed both upstream; they now guard those fixes.
 */
import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'

if (process.platform !== 'darwin') {
  console.log('skip: needs macOS WKWebView')
  process.exit(0)
}

const here = import.meta.dir
const root = path.resolve(here, '../..')
const work = mkdtempSync(path.join(tmpdir(), 'webgl-atlas-'))
const www = path.join(work, 'www')

execFileSync('bun', [path.join(here, 'build.ts'), www], { stdio: 'inherit' })
copyFileSync(path.join(here, 'index.html'), path.join(www, 'index.html'))
copyFileSync(
  path.join(root, 'node_modules/@xterm/xterm/css/xterm.css'),
  path.join(www, 'xterm.css')
)
for (const tool of ['runner', 'diff']) {
  execFileSync('swiftc', ['-O', path.join(here, `${tool}.swift`), '-o', path.join(work, tool)], {
    stdio: ['ignore', 'ignore', 'inherit']
  })
}

const server = Bun.serve({
  port: 0,
  hostname: '127.0.0.1',
  fetch(request) {
    const file = new URL(request.url).pathname.replace(/^\/$/, '/index.html')
    return new Response(Bun.file(path.join(www, path.normalize(file))))
  }
})

// Async on purpose: a blocking spawn would stall the event loop that serves the
// page the runner is waiting for.
async function render(name: string, query: string): Promise<{ png: string; merged: number }> {
  const png = path.join(work, `${name}.png`)
  const url = `http://127.0.0.1:${server.port}/index.html?${query}`
  const runner = Bun.spawn([path.join(work, 'runner'), url, png, path.join(work, `${name}.txt`)], {
    stdout: 'pipe',
    stderr: 'inherit'
  })
  const out = await new Response(runner.stdout).text()
  if ((await runner.exited) !== 0) throw new Error(`runner failed for ${name}`)
  const match = / merged=(\d+) /.exec(out)
  if (!match) throw new Error(`unexpected runner output for ${name}: ${out}`)
  return { png, merged: Number(match[1]) }
}

function differingPixels(a: string, b: string): number {
  const out = execFileSync(path.join(work, 'diff'), [a, b, `${b}.diff.png`]).toString()
  const match = /differing=(\d+)/.exec(out)
  if (!match) throw new Error(`unexpected diff output: ${out}`)
  return Number(match[1])
}

const hidden = 'scenario=hidden&screen=alt&terms=2&capture=2&frames=900&hide=visibility&hook=app'
// The detector control runs without the hide: after the hidden run's merges
// page 0 can be empty, and clearTextureAtlas is then a silent no-op upstream.
const sibling = 'scenario=tui&terms=2&capture=2'

const checks = [
  { name: 'fixed policy stays clean', scenario: hidden, extra: '' },
  { name: 'legacy policy stays clean (#6042)', scenario: hidden, extra: '&policy=legacy' },
  {
    name: 'sibling clearTextureAtlas stays clean (#6055)',
    scenario: sibling,
    extra: '&repair=sibling-atlas'
  }
]

const references = new Map<string, string>()
let failed = false
for (const [index, check] of checks.entries()) {
  let reference = references.get(check.scenario)
  if (!reference) {
    reference = (await render(`reference-${index}`, `mode=reference&${check.scenario}`)).png
    references.set(check.scenario, reference)
  }
  const stress = await render(`stress-${index}`, `mode=stress&${check.scenario}${check.extra}`)
  const differing = differingPixels(reference, stress.png)
  const ok = differing === 0 && stress.merged > 0
  if (!ok) failed = true
  console.log(
    `${ok ? 'PASS' : 'FAIL'} ${check.name}: ${differing} differing pixels, ${stress.merged} page merges`
  )
}

// Detector control: two different screens must differ, or 0 above proves nothing.
const [hiddenReference, siblingReference] = [hidden, sibling].map((s) => references.get(s) ?? '')
const detected = differingPixels(hiddenReference, siblingReference)
if (detected === 0) failed = true
console.log(
  `${detected > 0 ? 'PASS' : 'FAIL'} detector sees different screens: ${detected} differing pixels`
)

server.stop()
if (failed) {
  console.log(`artifacts: ${work}`)
}
process.exit(failed ? 1 : 0)
