import { describe, expect, it } from 'vitest'
import { beginConfigMutation, mergeCommandSnapshots, mergeTelemetryStatuses, mergeTransferSnapshots, mergeTunnelSnapshot, shouldApplyBootstrap, shouldApplyConfigBootstrap } from './store'
import type { CommandJob, TelemetryStatus, TransferJob, TunnelSnapshot } from './types'

function transfer(state: TransferJob['state'], updatedAt: string, revision = 1): TransferJob {
  return {
    id: 'transfer', revision, hostId: 'host', direction: 'upload', source: 'a', destination: 'b', state,
    bytesTransferred: state === 'completed' ? 10 : 1, totalBytes: 10, error: null,
    createdAt: '2026-01-01T00:00:00Z', updatedAt
  }
}

function command(state: CommandJob['state'], stdout: string, revision = 1): CommandJob {
  return {
    id: 'command', revision, commandId: null, hostId: 'host', name: 'test', command: 'uptime', risk: 'L1',
    state, stdout, stderr: '', exitCode: state === 'completed' ? 0 : null, error: null,
    startedAt: null, finishedAt: state === 'completed' ? '2026-01-01T00:00:02Z' : null
  }
}

describe('async task snapshot merges', () => {
  it('does not let a slow older bootstrap replace a faster configuration revision', async () => {
    let appliedRevision = 4
    let selectedAlias = 'new-host'
    let releaseOlder: ((value: { revision: number; alias: string }) => void) | undefined
    const older = new Promise<{ revision: number; alias: string }>((resolve) => { releaseOlder = resolve })
    const newer = Promise.resolve({ revision: 5, alias: 'new-host' })

    const fast = await newer
    if (shouldApplyBootstrap(appliedRevision, fast.revision)) {
      appliedRevision = fast.revision
      selectedAlias = fast.alias
    }
    releaseOlder?.({ revision: 4, alias: 'old-host' })
    const slow = await older
    if (shouldApplyBootstrap(appliedRevision, slow.revision)) {
      appliedRevision = slow.revision
      selectedAlias = slow.alias
    }

    expect(appliedRevision).toBe(5)
    expect(selectedAlias).toBe('new-host')
  })

  it('keeps the server revision when resync arrives before a configuration mutation response', () => {
    // A save starts while revision 4 is visible. Its resync snapshot can legitimately
    // arrive before the HTTP response, so the UI must not manufacture revision 6 later.
    const mutation = beginConfigMutation(4, 10)
    expect(mutation).toEqual({ epoch: 11, minimumRevision: 5 })
    expect(shouldApplyConfigBootstrap(4, mutation.epoch, mutation.epoch, mutation.minimumRevision, 5)).toBe(true)

    const revisionAfterResync = 5
    // The save response only applies its returned host object. Revision 5 remains
    // authoritative and the old pre-mutation request cannot subsequently apply.
    expect(revisionAfterResync).toBe(5)
    expect(shouldApplyConfigBootstrap(revisionAfterResync, mutation.epoch, 10, null, 4)).toBe(false)
    expect(shouldApplyBootstrap(revisionAfterResync, 5)).toBe(true)
  })

  it('does not let an older transfer list overwrite a newer event', () => {
    const current = transfer('completed', '2026-01-01T00:00:02Z', 4)
    const stale = transfer('running', '2026-01-01T00:00:03Z', 3)
    expect(mergeTransferSnapshots([current], [stale])).toEqual([current])
    expect(mergeTransferSnapshots([stale], [current])).toEqual([current])
  })

  it('does not let a stale tunnel list overwrite a newer event', () => {
    const current: TunnelSnapshot = {
      tunnelId: 'tunnel', revision: 7, state: 'running', message: null,
      logs: [{ at: '2026-01-01T00:00:00Z', level: 'info', message: 'connected' }]
    }
    const stale: TunnelSnapshot = { tunnelId: 'tunnel', revision: 6, state: 'starting', message: null }
    const states = { tunnel: current }
    expect(mergeTunnelSnapshot(states, stale)).toBe(states)
    expect(mergeTunnelSnapshot(states, { ...current, revision: 8, state: 'waiting' }).tunnel?.state).toBe('waiting')
    const summary = { ...current, revision: 8, logs: undefined, uptimeSeconds: 9 }
    expect(mergeTunnelSnapshot(states, summary).tunnel?.logs).toEqual(current.logs)
  })

  it('does not let a stale telemetry list overwrite a newer event', () => {
    const current: TelemetryStatus = { hostId: 'host', revision: 4, state: 'online', lastSampleAt: '2026-01-01T00:00:04Z', error: null }
    const stale: TelemetryStatus = { hostId: 'host', revision: 3, state: 'starting', lastSampleAt: null, error: null }
    const statuses = { host: current }
    expect(mergeTelemetryStatuses(statuses, [stale])).toBe(statuses)
    expect(mergeTelemetryStatuses(statuses, [{ ...current, revision: 5, state: 'stopped' }]).host?.state).toBe('stopped')
  })

  it('keeps terminal command state and the longest same-state output', () => {
    const completed = command('completed', 'done', 4)
    expect(mergeCommandSnapshots([completed], [command('running', 'd', 3)])).toEqual([completed])
    const current = command('running', 'partial output', 5)
    expect(mergeCommandSnapshots([current], [command('running', 'part', 5)])).toEqual([current])
  })

  it('bounds retained task history without evicting active work', () => {
    const oldActive = { ...transfer('running', '2026-01-01T00:00:00Z'), id: 'active' }
    const completed = Array.from({ length: 512 }, (_, index) => ({
      ...transfer('completed', `2026-01-01T00:00:${String(index % 60).padStart(2, '0')}Z`),
      id: `completed-${String(index)}`
    }))
    const mergedTransfers = mergeTransferSnapshots([oldActive, ...completed], [])
    expect(mergedTransfers).toHaveLength(512)
    expect(mergedTransfers.some((job) => job.id === oldActive.id)).toBe(true)

    const activeCommand = { ...command('running', ''), id: 'active-command' }
    const completedCommands = Array.from({ length: 512 }, (_, index) => ({
      ...command('completed', 'done'),
      id: `completed-command-${String(index)}`
    }))
    const mergedCommands = mergeCommandSnapshots([activeCommand, ...completedCommands], [])
    expect(mergedCommands).toHaveLength(512)
    expect(mergedCommands.some((job) => job.id === activeCommand.id)).toBe(true)
  })

  it('bounds retained terminal command output bytes', () => {
    const active = { ...command('running', ''), id: 'active-output' }
    const completed = Array.from({ length: 17 }, (_, index) => ({
      ...command('completed', 'x'.repeat(1024 * 1024)),
      id: `large-command-${String(index)}`
    }))
    const merged = mergeCommandSnapshots([active, ...completed], [])
    expect(merged.some((job) => job.id === active.id)).toBe(true)
    expect(merged.reduce((bytes, job) => bytes + job.stdout.length + job.stderr.length, 0)).toBeLessThanOrEqual(16 * 1024 * 1024)
  })
})
