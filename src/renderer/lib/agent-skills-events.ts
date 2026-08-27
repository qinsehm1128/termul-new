export const AGENT_SKILLS_CHANGED_EVENT = 'termul:agent-skills-changed'

export interface AgentSkillsChangedDetail {
  root: string
}

export function notifyAgentSkillsChanged(root: string): void {
  const normalized = root.trim()
  if (!normalized || typeof window === 'undefined') return
  window.dispatchEvent(
    new CustomEvent<AgentSkillsChangedDetail>(AGENT_SKILLS_CHANGED_EVENT, {
      detail: { root: normalized }
    })
  )
}
