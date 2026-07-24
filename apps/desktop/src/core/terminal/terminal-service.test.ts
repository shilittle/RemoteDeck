import { Duplex, PassThrough } from 'node:stream'
import { describe, expect, it } from 'vitest'
import type { TerminalEvent } from '../../protocol/terminal'
import { TerminalService } from './terminal-service'

class FakeChannel extends Duplex {
  readonly stderr = new PassThrough()
  readonly writes: string[] = []
  resize: [number, number, number, number] | undefined
  _read(): void { /* test stream is pushed explicitly */ }
  _write(chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null) => void): void { this.writes.push(chunk.toString()); callback() }
  setWindow(rows: number, cols: number, height: number, width: number): void { this.resize = [rows, cols, height, width] }
}

describe('terminal PTY lifecycle', () => {
  it('streams UTF-8 bidirectionally, resizes, marks disconnect, and reconnects as a new shell', async () => {
    const channels: FakeChannel[] = []
    const client = {
      shell: (_options: unknown, callback: (error: Error | undefined, channel: FakeChannel) => void) => {
        const channel = new FakeChannel()
        channels.push(channel)
        callback(undefined, channel)
      }
    }
    const profile = {
      host: { id: '019f92f0-b87c-7c74-a668-54d5b18fb487', alias: '测试机' },
      workspace: { remotePath: '/tmp/中文 路径' }
    }
    const service = new TerminalService({ getOnlineClient: () => client } as never, { get: () => Promise.resolve(profile) } as never)
    const events: TerminalEvent[] = []
    service.on('event', (event: TerminalEvent) => events.push(event))

    const session = await service.create({ hostId: profile.host.id, cols: 80, rows: 24 })
    expect(session).toMatchObject({ hostAlias: '测试机', cwd: '/tmp/中文 路径', state: 'online', generation: 1 })
    expect(channels[0]?.writes[0]).toContain("cd -- '/tmp/中文 路径'")
    channels[0]?.push('你好，PTY\r\n')
    await new Promise((resolve) => setImmediate(resolve))
    expect(events).toContainEqual(expect.objectContaining({ type: 'data', sessionId: session.id, data: '你好，PTY\r\n' }))

    service.write(session.id, '\u0003')
    expect(channels[0]?.writes).toContain('\u0003')
    expect(service.resize({ sessionId: session.id, cols: 132, rows: 43 })).toMatchObject({ cols: 132, rows: 43 })
    expect(channels[0]?.resize).toEqual([43, 132, 0, 0])

    channels[0]?.emit('exit', 255, 'HUP')
    channels[0]?.emit('close')
    expect(service.list()[0]).toMatchObject({ state: 'offline', exitCode: 255, exitSignal: 'HUP' })

    const reconnected = await service.reconnect(session.id)
    expect(reconnected).toMatchObject({ state: 'online', generation: 2, cols: 132, rows: 43 })
    expect(channels).toHaveLength(2)
    expect(service.close(session.id).state).toBe('closed')
    expect(service.list()).toHaveLength(0)
  })
})
