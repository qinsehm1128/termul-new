import { useEffect } from 'react'
import { useMcpStore } from '@/stores/mcp-store'

/** Load the project MCP control plane once at app mount. */
export function useAcpMcp(): void {
  const load = useMcpStore((s) => s.load)
  useEffect(() => {
    void load()
  }, [load])
}
