import { mkdirSync, readdirSync, statSync, unlinkSync } from 'node:fs'
import { join } from 'node:path'
import pino, { type Logger, type LevelWithSilent } from 'pino'
import { redactSecrets } from '../../core/logging/redaction'

export type LogCategory = 'main' | 'ssh' | 'tunnel' | 'transfer' | 'telemetry' | 'command'

export function createAppLogger(logDirectory: string, level: LevelWithSilent = 'info'): Logger {
  mkdirSync(logDirectory, { recursive: true })
  const sessionStamp = new Date().toISOString().replaceAll(':', '').replaceAll('.', '')
  const logPath = join(logDirectory, `remotedeck-${sessionStamp}-${String(process.pid)}.log`)
  const logger = pino({
    level,
    base: { app: 'RemoteDeck' },
    timestamp: pino.stdTimeFunctions.isoTime,
    hooks: {
      logMethod(args, method) {
        method.apply(this, args.map((value) => redactSecrets(value)) as Parameters<typeof method>)
      }
    }
  }, pino.destination({ dest: logPath, mkdir: true, sync: false }))
  pruneLogFiles(logDirectory)
  return logger
}

export function categoryLogger(logger: Logger, category: LogCategory): Logger {
  return logger.child({ category })
}

export function pruneLogFiles(logDirectory: string, keepFiles = 20): void {
  const files = readdirSync(logDirectory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && /^remotedeck-[\w.-]+\.log$/i.test(entry.name))
    .map((entry) => ({ path: join(logDirectory, entry.name), modified: statSync(join(logDirectory, entry.name)).mtimeMs }))
    .sort((left, right) => right.modified - left.modified)
  for (const file of files.slice(Math.max(1, keepFiles))) {
    try { unlinkSync(file.path) } catch { /* A locked log is retained and retried at the next launch. */ }
  }
}
