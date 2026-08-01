import { describe, expect, it } from 'vitest'
import {
  MAX_TELEMETRY_HISTORY_BYTES,
  MAX_TELEMETRY_HISTORY_SAMPLES,
  appendTelemetrySample,
  retainTelemetryHistory,
  telemetrySnapshotBytes
} from './telemetry-history'
import type { TelemetrySnapshot } from './types'

function sample(sequence: number, command = `process-${String(sequence)}`): TelemetrySnapshot {
  return {
    hostId: 'host',
    sampledAt: `2026-08-02T00:00:${String(sequence).padStart(2, '0')}Z`,
    hostname: 'lab',
    currentUser: 'alice',
    cpu: { percent: sequence, load1: 0, load5: 0, load15: 0 },
    memory: { usedBytes: 1, totalBytes: 2, swapUsedBytes: 0, swapTotalBytes: 0 },
    network: { receiveBytesPerSecond: 0, sendBytesPerSecond: 0 },
    disks: [],
    gpus: [],
    processes: [{
      pid: sequence + 1,
      user: 'alice',
      cpuPercent: 0,
      memoryPercent: 0,
      state: 'S',
      elapsedSeconds: sequence,
      startTicks: sequence + 100,
      command
    }]
  }
}

describe('frontend telemetry history retention', () => {
  it('uses the 1200-sample and 16 MiB frontend budgets', () => {
    expect(MAX_TELEMETRY_HISTORY_SAMPLES).toBe(1200)
    expect(MAX_TELEMETRY_HISTORY_BYTES).toBe(16 * 1024 * 1024)
  })

  it('keeps only the newest samples under the count limit', () => {
    const retained = retainTelemetryHistory(
      [sample(1), sample(2), sample(3), sample(4)],
      { maxSamples: 2, maxBytes: 1024 * 1024 }
    )
    expect(retained.map((item) => item.cpu.percent)).toEqual([3, 4])
  })

  it('enforces the UTF-8 byte budget and favors the newest sample', () => {
    const older = sample(1, '界'.repeat(200))
    const newer = sample(2, '界'.repeat(200))
    const oneSampleBudget = Math.max(telemetrySnapshotBytes(older), telemetrySnapshotBytes(newer))
    const retained = retainTelemetryHistory(
      [older, newer],
      { maxSamples: 10, maxBytes: oneSampleBudget }
    )
    expect(retained).toEqual([newer])
    expect(telemetrySnapshotBytes(newer)).toBeGreaterThan(newer.processes[0]?.command.length ?? 0)
  })

  it('replaces a duplicate host/timestamp with the newest payload', () => {
    const previous = sample(1)
    const replacement = { ...sample(9), sampledAt: previous.sampledAt }
    const retained = appendTelemetrySample(
      [previous],
      replacement,
      { maxSamples: 10, maxBytes: 1024 * 1024 }
    )
    expect(retained).toEqual([replacement])
  })

  it('drops an individual sample that exceeds the whole byte budget', () => {
    const oversized = sample(1, 'x'.repeat(4096))
    expect(retainTelemetryHistory(
      [oversized],
      { maxSamples: 10, maxBytes: telemetrySnapshotBytes(oversized) - 1 }
    )).toEqual([])
  })
})
