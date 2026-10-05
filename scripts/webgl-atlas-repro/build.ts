// Bundle the repro page. The repair module logs through Tauri IPC, which the
// page does not have, so `@/lib/log-api` resolves to a no-op stub.
import path from 'node:path'

const outdir = process.argv[2] ?? '/tmp/atlas-repro/www'
const result = await Bun.build({
  entrypoints: [path.join(import.meta.dir, 'harness.ts')],
  outdir,
  target: 'browser',
  format: 'esm',
  plugins: [
    {
      name: 'stub-log-api',
      setup(build) {
        build.onResolve({ filter: /^@\/lib\/log-api$/ }, () => ({
          path: path.join(import.meta.dir, 'log-api-stub.ts')
        }))
      }
    }
  ]
})
if (!result.success) {
  for (const log of result.logs) console.error(log)
  process.exit(1)
}
console.log('built', result.outputs.map((o) => o.path).join(', '))
