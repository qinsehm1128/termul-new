/**
 * Web/remote HTTP adapter for the memory index.
 */
import type { MemoryIndexApi } from '@shared/types/memory-index.types'

import { webServerMemoryIndex } from './web-server-api'

export const webMemoryIndexApi: MemoryIndexApi = {
  build(args) {
    return webServerMemoryIndex.build(args)
  },
  status(args) {
    return webServerMemoryIndex.status(args)
  },
  search(args) {
    return webServerMemoryIndex.search(args)
  },
  listSessions(args) {
    return webServerMemoryIndex.listSessions(args)
  },
  getSession(args) {
    return webServerMemoryIndex.getSession(args)
  },
  cancel(args) {
    return webServerMemoryIndex.cancel(args)
  },
  /**
   * Not available over HTTP, and `null` rather than a guess.
   *
   * The invocation names the *host's* executable path and state root. A browser
   * client cannot run either, and a plausible-looking path that does not exist
   * on the machine reading it is worse than an honest absence.
   */
  async mcpInvocation() {
    return null
  },
  async universalMcpInvocation() {
    return null
  }
}
