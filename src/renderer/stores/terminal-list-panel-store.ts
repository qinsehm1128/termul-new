import { create } from 'zustand'

interface TerminalListPanelState {
  isVisible: boolean
  toggleVisibility: () => void
  setVisible: (visible: boolean) => void
}

/**
 * Live visibility of the vertical terminal list docked at the right edge of
 * the pane area. Mirrors `AppSettings.terminalListPanelVisible`, which is the
 * persisted authority — never write here directly from UI, go through
 * `useUpdatePanelVisibility` so the value survives a restart.
 */
export const useTerminalListPanelStore = create<TerminalListPanelState>((set) => ({
  isVisible: false,

  toggleVisibility: (): void => {
    set((state) => ({ isVisible: !state.isVisible }))
  },

  setVisible: (visible: boolean): void => {
    set({ isVisible: visible })
  }
}))

export function useTerminalListPanelVisible(): boolean {
  return useTerminalListPanelStore((state) => state.isVisible)
}
