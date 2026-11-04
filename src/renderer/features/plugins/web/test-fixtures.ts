import type { PluginWebSession } from '../plugin-api'

export const webSession: PluginWebSession = {
  bootId: '44444444-4444-4444-8444-444444444444',
  sessionId: '11111111-1111-4111-8111-111111111111',
  nonce: '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
  pluginId: 'org.termul.example',
  generation: '22222222-2222-4222-8222-222222222222',
  viewId: 'web',
  entry: 'ui/index.html',
  packageDigest: 'a'.repeat(64),
  allow: ['commands', 'view.dispatch', 'config.read', 'config.write'],
  shimVersion: 1,
  url: 'se-plugin://localhost/org.termul.example/22222222-2222-4222-8222-222222222222/11111111-1111-4111-8111-111111111111/ui/index.html'
}
