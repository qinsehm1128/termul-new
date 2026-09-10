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
  }
}
