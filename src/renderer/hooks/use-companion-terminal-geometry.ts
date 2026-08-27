import type { TerminalDisplayMode } from '@shared/types/ipc.types'
import { createContext, useContext } from 'react'

export interface CompanionTerminalGeometry {
  surfaceActive: boolean
  preferredMode: TerminalDisplayMode
  keyboardOpen: boolean
  setPreferredMode: (mode: TerminalDisplayMode) => void
}

export const CompanionTerminalGeometryContext = createContext<CompanionTerminalGeometry | null>(
  null
)

export function useCompanionTerminalGeometry(): CompanionTerminalGeometry | null {
  return useContext(CompanionTerminalGeometryContext)
}
