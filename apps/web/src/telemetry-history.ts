import type { TelemetrySnapshot } from './types'

export const MAX_TELEMETRY_HISTORY_SAMPLES = 1200
export const MAX_TELEMETRY_HISTORY_BYTES = 16 * 1024 * 1024

export interface TelemetryHistoryLimits {
  maxSamples: number
  maxBytes: number
}

const DEFAULT_LIMITS: TelemetryHistoryLimits = {
  maxSamples: MAX_TELEMETRY_HISTORY_SAMPLES,
  maxBytes: MAX_TELEMETRY_HISTORY_BYTES
}

const encoder = new TextEncoder()

export function telemetrySnapshotBytes(sample: TelemetrySnapshot): number {
  return encoder.encode(JSON.stringify(sample)).byteLength
}

export function retainTelemetryHistory(
  samples: readonly TelemetrySnapshot[],
  limits: TelemetryHistoryLimits = DEFAULT_LIMITS
): TelemetrySnapshot[] {
  const maxSamples = boundedLimit(limits.maxSamples)
  const maxBytes = boundedLimit(limits.maxBytes)
  if (maxSamples === 0 || maxBytes === 0) return []

  const seen = new Set<string>()
  const newestFirst: TelemetrySnapshot[] = []
  let retainedBytes = 0

  for (let index = samples.length - 1; index >= 0 && newestFirst.length < maxSamples; index -= 1) {
    const sample = samples[index]
    const identity = `${sample.hostId}\u0000${sample.sampledAt}`
    if (seen.has(identity)) continue
    seen.add(identity)

    const sampleBytes = telemetrySnapshotBytes(sample)
    if (sampleBytes > maxBytes - retainedBytes) continue
    newestFirst.push(sample)
    retainedBytes += sampleBytes
  }

  return newestFirst.reverse()
}

export function appendTelemetrySample(
  current: readonly TelemetrySnapshot[],
  sample: TelemetrySnapshot,
  limits: TelemetryHistoryLimits = DEFAULT_LIMITS
): TelemetrySnapshot[] {
  return retainTelemetryHistory([...current, sample], limits)
}

function boundedLimit(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0
}
