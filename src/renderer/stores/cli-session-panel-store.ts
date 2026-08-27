import { create } from 'zustand'

interface CliSessionPanelState {
  isVisible: boolean
  toggleVisibility: () => void
  setVisible: (visible: boolean) => void
}

export const useCliSessionPanelStore = create<CliSessionPanelState>((set) => ({
  isVisible: false,

  toggleVisibility: (): void => {
    set((state) => ({ isVisible: !state.isVisible }))
  },

  setVisible: (visible: boolean): void => {
    set({ isVisible: visible })
  }
}))

export function useCliSessionPanelVisible(): boolean {
  return useCliSessionPanelStore((state) => state.isVisible)
}
