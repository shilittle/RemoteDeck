import type { Client, ClientChannel } from 'ssh2'

export interface RemoteExecResult { stdout: string; stderr: string; code: number | null; signal: string | null }

export async function remoteExec(client: Client, command: string, timeoutMs = 15_000, maxBytes = 1024 * 1024): Promise<RemoteExecResult> {
  return new Promise((resolve, reject) => {
    let settled = false
    let channel: ClientChannel | undefined
    let stdout = ''
    let stderr = ''
    let code: number | null = null
    let signal: string | null = null
    const timer = setTimeout(() => { channel?.close(); finish(new Error('Remote command timed out')) }, timeoutMs)
    const finish = (error?: Error): void => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      if (error) reject(error)
      else resolve({ stdout, stderr, code, signal })
    }
    client.exec(command, (error, stream) => {
      if (error) { finish(error); return }
      channel = stream
      configure(stream)
    })
    function configure(stream: ClientChannel): void {
      stream.setEncoding('utf8')
      stream.stderr.setEncoding('utf8')
      stream.on('data', (chunk: string) => { stdout = appendBounded(stdout, chunk, maxBytes) })
      stream.stderr.on('data', (chunk: string) => { stderr = appendBounded(stderr, chunk, maxBytes) })
      stream.once('exit', (nextCode?: number, nextSignal?: string) => { code = nextCode ?? null; signal = nextSignal ?? null })
      stream.once('error', finish)
      stream.once('close', () => finish())
    }
  })
}

function appendBounded(current: string, chunk: string, maxBytes: number): string {
  const next = current + chunk
  if (Buffer.byteLength(next) <= maxBytes) return next
  return Buffer.from(next).subarray(-maxBytes).toString('utf8')
}

export function shellQuote(value: string): string { return `'${value.replaceAll("'", `'"'"'`)}'` }

export function inDirectory(command: string, cwd?: string): string {
  if (!cwd || cwd === '~') return command
  const target = cwd.startsWith('~/') ? `"$HOME"/${shellQuote(cwd.slice(2))}` : shellQuote(cwd)
  return `cd -- ${target} && ${command}`
}
