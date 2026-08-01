import { describe, expect, it } from 'vitest'
import {
  agentActionNeedsConfirmation,
  buildAgentSessionRequest,
  isCurrentAgentStatus
} from './agent-contract'
import type { AgentStatus } from './types'

const hostId = '11111111-2222-4333-8444-555555555555'

describe('agent lifecycle request contract', () => {
  it('sends a tmux selector only for resume', () => {
    const base = {
      hostId,
      agent: 'codex' as const,
      workspace: '/srv/project',
      sessionName: '  remotedeck-codex-existing  ',
      confirmation: null
    }
    expect(buildAgentSessionRequest({ ...base, action: 'resume' }).sessionName)
      .toBe('remotedeck-codex-existing')
    expect(buildAgentSessionRequest({ ...base, action: 'start' }).sessionName).toBeNull()
  })

  it('preserves exact confirmation only for install and update', () => {
    expect(agentActionNeedsConfirmation('install')).toBe(true)
    expect(agentActionNeedsConfirmation('update')).toBe(true)
    expect(buildAgentSessionRequest({
      hostId,
      agent: 'codex',
      action: 'install',
      workspace: null,
      sessionName: '',
      confirmation: 'Lab-A'
    }).confirmation).toBe('Lab-A')
    expect(buildAgentSessionRequest({
      hostId,
      agent: 'codex',
      action: 'login',
      workspace: null,
      sessionName: '',
      confirmation: 'stale-value'
    }).confirmation).toBeNull()
  })

  it('rejects stale probe state at the render boundary', () => {
    const status = {
      hostId,
      agent: 'codex',
      installed: true,
      version: 'codex 1.2.3',
      authenticated: true,
      tmuxAvailable: true,
      resumableSessions: [],
      installHint: null,
      documentationUrl: null
    } satisfies AgentStatus
    expect(isCurrentAgentStatus(status, hostId, 'codex')).toBe(true)
    expect(isCurrentAgentStatus(status, hostId, 'claude')).toBe(false)
    expect(isCurrentAgentStatus(status, 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee', 'codex')).toBe(false)
  })
})
