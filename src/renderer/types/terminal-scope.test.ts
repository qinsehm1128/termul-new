import { describe, expect, it } from 'vitest'
import { isConversationScopedTerminal, isProjectScopedTerminal } from './project'

/**
 * The two kinds of terminal are separate things, and this is the seam that says
 * so. Before it, "belongs to project X" was spelled `projectId === X` in seven
 * places and `projectId === X && !conversationId` in one — so a Conversation's
 * terminal was a project terminal to most of the app and not to the rest. That
 * disagreement is what let each kind leak into the other's workspace.
 */
describe('project vs conversation terminal scope', () => {
  const projectShell = { projectId: 'p1', conversationId: undefined }
  const conversationShell = { projectId: 'p1', conversationId: 'c1' }

  it("counts a project shell as the project's", () => {
    expect(isProjectScopedTerminal(projectShell, 'p1')).toBe(true)
  })

  it("does not count a Conversation terminal as the project's", () => {
    // It carries the project id for attribution — labelling and grouping — not
    // ownership. Reading that id as ownership is the whole bug.
    expect(conversationShell.projectId).toBe('p1')
    expect(isProjectScopedTerminal(conversationShell, 'p1')).toBe(false)
  })

  it("does not count another project's shell", () => {
    expect(isProjectScopedTerminal(projectShell, 'p2')).toBe(false)
  })

  it('treats the two predicates as complements within a project', () => {
    // Every terminal attributed to p1 is owned by either the project or a
    // Conversation, never both and never neither.
    for (const terminal of [projectShell, conversationShell]) {
      expect(isProjectScopedTerminal(terminal, 'p1')).toBe(!isConversationScopedTerminal(terminal))
    }
  })

  it('leaves an unassigned terminal out of every project', () => {
    expect(isProjectScopedTerminal({ projectId: undefined, conversationId: undefined }, 'p1')).toBe(
      false
    )
  })
})
