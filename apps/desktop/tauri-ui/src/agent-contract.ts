import type { AgentAction, AgentKind, AgentSessionRequest, AgentStatus, Identifier } from './types'

interface AgentSessionRequestInput {
  hostId: Identifier
  agent: AgentKind
  action: AgentAction
  workspace: string | null
  sessionName: string
  confirmation: string | null
}

export function agentActionNeedsConfirmation(action: AgentAction): boolean {
  return action === 'install' || action === 'update'
}

export function buildAgentSessionRequest(input: AgentSessionRequestInput): AgentSessionRequest {
  return {
    hostId: input.hostId,
    agent: input.agent,
    action: input.action,
    workspace: input.workspace,
    sessionName: input.action === 'resume' ? input.sessionName.trim() || null : null,
    confirmation: agentActionNeedsConfirmation(input.action) ? input.confirmation : null
  }
}

export function isCurrentAgentStatus(
  status: AgentStatus | null,
  hostId: Identifier,
  agent: AgentKind
): status is AgentStatus {
  return status?.hostId === hostId && status.agent === agent
}
