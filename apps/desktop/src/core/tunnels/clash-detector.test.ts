import { createServer } from 'node:net'
import type { Server } from 'node:net'
import { afterEach, describe, expect, it } from 'vitest'
import { probeProxyPort } from './clash-detector'

const servers: Server[] = []

afterEach(async () => {
  await Promise.all(servers.splice(0).map((server) => new Promise<void>((resolve) => server.close(() => resolve()))))
})

describe('Clash listener protocol probes', () => {
  it('identifies a SOCKS5 greeting before generic TCP', async () => {
    const port = await listen(createServer((socket) => socket.once('data', () => socket.end(Buffer.from([5, 0])))))
    await expect(probeProxyPort(port)).resolves.toMatchObject({ protocol: 'socks5', confidence: 'high' })
  })

  it('identifies HTTP CONNECT and plain TCP candidates', async () => {
    const httpPort = await listen(createServer((socket) => socket.once('data', () => socket.end('HTTP/1.1 502 Probe\r\n\r\n'))))
    await expect(probeProxyPort(httpPort)).resolves.toMatchObject({ protocol: 'http-connect', confidence: 'high' })
    const tcpPort = await listen(createServer((socket) => socket.once('data', () => undefined)))
    await expect(probeProxyPort(tcpPort)).resolves.toMatchObject({ protocol: 'tcp', confidence: 'medium' })
  })
})

async function listen(server: Server): Promise<number> {
  servers.push(server)
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', () => resolve()) })
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('Test server has no TCP port')
  return address.port
}
