/** Wire contracts shared by desktop and authenticated browser agent-chat transports. */
export type AgentChatControlAction = 'interrupt' | 'stop'
export type AgentChatDeliveryStatus = 'queued' | 'pending' | 'delivered' | 'unconfirmed'

export interface AgentChatDelivery {
  id: string
  text: string
  status: AgentChatDeliveryStatus
}

/** Message is specialized to the existing runtime ChatMessage by the renderer. */
export interface AgentChatSnapshot<Message = unknown, Tool = unknown> {
  terminal_id: string
  session_id: string | null
  agent: string | null
  cursor: number
  /** Exclusive message-sequence boundary for the next older disk page; not the live cursor. */
  older_cursor?: number | null
  messages: Message[]
  tool_calls: Tool[]
  reset: boolean
  deliveries: AgentChatDelivery[]
  state: 'ready' | 'degraded'
  reason?: string
  hook_state: string
}

/** Notifications contain metadata only; transcript content is fetched via snapshot. */
export interface AgentChatUpdated {
  terminal_id: string
  cursor: number
}

export interface AgentChatApi<Message = unknown, Tool = unknown> {
  snapshot(
    terminalId: string,
    after?: number,
    before?: number
  ): Promise<AgentChatSnapshot<Message, Tool>>
  send(terminalId: string, text: string): Promise<unknown>
  control(terminalId: string, action: AgentChatControlAction): Promise<unknown>
}
