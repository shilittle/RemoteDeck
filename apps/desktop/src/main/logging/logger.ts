import { mkdirSync } from 'node:fs'
import { join } from 'node:path'
import pino, { type Logger, type LevelWithSilent } from 'pino'
import { redactSecrets } from '../../core/logging/redaction'

export type LogCategory = 'main' | 'ssh' | 'tunnel' | 'transfer' | 'telemetry' | 'command'

export function createAppLogger(logDirectory: string, level: LevelWithSilent = 'info'): Logger {
  mkdirSync(logDirectory, { recursive: true })
  const logPath = join(logDirectory, `remotedeck-${new Date().toISOString().slice(0, 10)}.log`)
  return pino({
    level,
    base: { app: 'RemoteDeck' },
    timestamp: pino.stdTimeFunctions.isoTime,
    hooks: {
      logMethod(args, method) {
        method.apply(this, args.map((value) => redactSecrets(value)) as Parameters<typeof method>)
      }
    }
  }, pino.destination({ dest: logPath, mkdir: true, sync: false }))
}

export function categoryLogger(logger: Logger, category: LogCategory): Logger {
  return logger.child({ category })
}

