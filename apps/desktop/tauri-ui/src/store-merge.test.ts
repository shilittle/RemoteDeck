import { describe, expect, it } from 'vitest'
import { mergeCommandSnapshots, mergeTelemetryStatuses, mergeTransferSnapshots, mergeTunnelSnapshot } from './store'
import type { CommandJob, TelemetryStatus, TransferJob, TunnelSnapshot } from './types'

function transfer(state: TransferJob['state'], updatedAt: string): TransferJob {
  return {
    id: 'transfer', hostId: 'host', direction: 'upload', source: 'a', destination: 'b', state,
    bytesTransferred: state === 'completed' ? 10 : 1, totalBytes: 10, error: null,
    createdAt: '2026-01-01T00:00:00Z', updatedAt
  }
}

function command(state: CommandJob['state'], stdout: string): CommandJob {
  return {
    id: 'command', commandId: null, hostId: 'host', name: 'test', command: 'uptime', risk: 'L1',
    state, stdout, stderr: '', exitCode: state === 'completed' ? 0 : null, error: null,
    startedAt: null, finishedAt: state === 'completed' ? '2026-01-01T00:00:02Z' : null
  }
}

describe('async task snapshot merges', () => {
  it('does not let an older transfer list overwrite a newer event', () => {
    const current = transfer('completed', '2026-01-01T00:00:02Z')
    const stale = transfer('running', '2026-01-01T00:00:01Z')
    expect(mergeTransferSnapshots([current], [stale])).toEqual([current])
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
    const completed = command('completed', 'done')
    expect(mergeCommandSnapshots([completed], [command('running', 'd')])).toEqual([completed])
    const current = command('running', 'partial output')
    expect(mergeCommandSnapshots([current], [command('running', 'part')])).toEqual([current])
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
