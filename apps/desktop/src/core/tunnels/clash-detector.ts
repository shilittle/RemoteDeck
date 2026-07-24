import { execFile } from 'node:child_process'
import { Socket } from 'node:net'
import { promisify } from 'node:util'
import type { ClashCandidate } from '../../protocol/tunnel'

const execFileAsync = promisify(execFile)
const proxyProcessPattern = /(?:clash|mihomo)/i

interface ListenerRecord { processName: string; pid: number; address: string; port: number }

export async function detectClashCandidates(): Promise<ClashCandidate[]> {
  if (process.platform !== 'win32') return []
  const command = "$items = Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.LocalAddress -in @('127.0.0.1','::1','0.0.0.0','::') } | ForEach-Object { $p = Get-Process -Id $_.OwningProcess -ErrorAction SilentlyContinue; if ($p) { [pscustomobject]@{ processName=$p.ProcessName; pid=$_.OwningProcess; address=$_.LocalAddress; port=$_.LocalPort } } }; @($items) | ConvertTo-Json -Compress"
  const { stdout } = await execFileAsync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', command], { windowsHide: true, timeout: 10_000, maxBuffer: 2 * 1024 * 1024 })
  const raw: unknown = stdout.trim() ? JSON.parse(stdout) : []
  const values = Array.isArray(raw) ? raw : [raw]
  const records = values.flatMap((value): ListenerRecord[] => {
    if (!value || typeof value !== 'object') return []
    const item = value as Record<string, unknown>
    const processName = typeof item['processName'] === 'string' ? item['processName'] : ''
    const pid = Number(item['pid'])
    const port = Number(item['port'])
    const address = typeof item['address'] === 'string' ? item['address'] : '127.0.0.1'
    return proxyProcessPattern.test(processName) && Number.isInteger(pid) && pid > 0 && Number.isInteger(port) && port > 0 && port <= 65_535 ? [{ processName, pid, port, address }] : []
  })
  const unique = [...new Map(records.map((item) => [`${String(item.pid)}:${String(item.port)}`, item])).values()]
  return (await Promise.all(unique.map(async (item) => ({ ...item, ...await probeProxyPort(item.port) })))).sort((left, right) => confidenceRank(right.confidence) - confidenceRank(left.confidence) || left.port - right.port)
}

export async function probeProxyPort(port: number): Promise<Pick<ClashCandidate, 'protocol' | 'confidence' | 'detail'>> {
  if (await probeSocks5(port)) return { protocol: 'socks5', confidence: 'high', detail: 'SOCKS5 greeting succeeded' }
  if (await probeHttpConnect(port)) return { protocol: 'http-connect', confidence: 'high', detail: 'HTTP CONNECT response detected' }
  if (await probeTcp(port)) return { protocol: 'tcp', confidence: 'medium', detail: 'Loopback TCP listener accepted a connection' }
  return { protocol: 'tcp', confidence: 'low', detail: 'Listener disappeared or did not respond' }
}

function probeSocks5(port: number): Promise<boolean> { return exchange(port, Buffer.from([5, 1, 0]), (data) => data.length >= 2 && data[0] === 5 && data[1] === 0) }
function probeHttpConnect(port: number): Promise<boolean> { return exchange(port, Buffer.from('CONNECT 127.0.0.1:1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\n\r\n'), (data) => data.toString('ascii').startsWith('HTTP/1.')) }
function probeTcp(port: number): Promise<boolean> { return exchange(port, undefined, () => true, true) }

function exchange(port: number, payload: Buffer | undefined, validate: (data: Buffer) => boolean, connectOnly = false): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = new Socket()
    let settled = false
    const finish = (value: boolean): void => { if (settled) return; settled = true; clearTimeout(timer); socket.destroy(); resolve(value) }
    const timer = setTimeout(() => finish(false), 900)
    socket.once('error', () => finish(false))
    socket.connect(port, '127.0.0.1', () => { if (connectOnly) finish(true); else if (payload) socket.write(payload) })
    socket.on('data', (data: Buffer) => finish(validate(data)))
  })
}

function confidenceRank(value: ClashCandidate['confidence']): number { return value === 'high' ? 3 : value === 'medium' ? 2 : 1 }
