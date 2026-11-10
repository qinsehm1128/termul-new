import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

/**
 * The desktop HTML/audio/video preview loads files through Tauri's asset
 * protocol. Tauri's default glob matching refuses any path component that
 * starts with a dot on Unix, so files under `.claude/`, `.qin-code/` and other
 * hidden project folders rendered as a blank page. The scope therefore opts out
 * of literal-leading-dot matching and instead denies credential stores
 * explicitly; deny wins over allow in Tauri's scope.
 */
interface AssetScope {
  allow: string[]
  deny: string[]
  requireLiteralLeadingDot: boolean
}

function assetScope(): AssetScope {
  const conf = JSON.parse(readFileSync(join(process.cwd(), 'src-tauri/tauri.conf.json'), 'utf8'))
  return conf.app.security.assetProtocol.scope as AssetScope
}

describe('asset protocol scope', () => {
  it('lets previews load files inside hidden folders', () => {
    const scope = assetScope()
    expect(scope.requireLiteralLeadingDot).toBe(false)
    expect(scope.allow).toContain('$HOME/**')
  })

  it('keeps credential stores out of reach', () => {
    const { deny } = assetScope()
    for (const path of [
      '$HOME/.ssh/**',
      '$HOME/.gnupg/**',
      '$HOME/.aws/**',
      '$HOME/.kube/**',
      '$HOME/.docker/**',
      '$HOME/.config/gh/**',
      '$HOME/.netrc',
      '$HOME/.git-credentials',
      '$HOME/.npmrc',
      '$HOME/Library/Keychains/**'
    ]) {
      expect(deny).toContain(path)
    }
  })
})
