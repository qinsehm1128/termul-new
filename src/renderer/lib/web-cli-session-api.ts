/**
 * Web/remote HTTP adapter for CLI session discovery.
 */
import type { CliSessionApi } from '@shared/types/cli-session.types'

import { webServerCliSessions } from './web-server-api'

export const webCliSessionApi: CliSessionApi = {
  listSessions(args) {
    return webServerCliSessions.list(args)
  },
  resolveSessions(args) {
    return webServerCliSessions.resolve(args)
  }
}
