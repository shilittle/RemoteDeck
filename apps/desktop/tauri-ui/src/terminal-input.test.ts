import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import {
  MAX_TERMINAL_INPUT_BATCH_BYTES,
  MAX_TERMINAL_INPUT_FRAME_BYTES,
  sendTerminalInput
} from './api'

class FakeSocket {
  readyState = 1
  bufferedAmount = 0
  readonly frames: Uint8Array[] = []

  send(data: ArrayBuffer): void {
    this.frames.push(new Uint8Array(data))
  }
}

describe('terminal input transport', () => {
  it('does not expose terminal bytes through the Tauri invoke surface', () => {
    const source = readFileSync(new URL('./api.ts', import.meta.url), 'utf8')
    expect(source).toContain("invoke('open_terminal_input', { sessionId })")
    expect(source).not.toContain('write_terminal')
    expect(source).not.toMatch(/invoke\([^\n]+\{\s*sessionId,\s*data\s*\}/)
  })

  it('allows WebSocket connections only to IPv4 loopback in the packaged CSP', () => {
    const config = JSON.parse(readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8')) as {
      app: { security: { csp: string } }
    }
    const connect = config.app.security.csp
      .split(';')
      .map((directive) => directive.trim())
      .find((directive) => directive.startsWith('connect-src '))
    const websocketSources = connect?.split(/\s+/).filter((source) => source.startsWith('ws:'))
    expect(websocketSources).toEqual(['ws://127.0.0.1:*'])
  })

  it('sends UTF-8 bytes in bounded binary WebSocket frames', () => {
    const socket = new FakeSocket()
    const input = `密码🙂${'界'.repeat(30_000)}`

    sendTerminalInput(socket, input)

    expect(socket.frames.length).toBeGreaterThan(1)
    expect(socket.frames.every((frame) => frame.byteLength <= MAX_TERMINAL_INPUT_FRAME_BYTES)).toBe(true)
    const size = socket.frames.reduce((total, frame) => total + frame.byteLength, 0)
    const combined = new Uint8Array(size)
    let offset = 0
    for (const frame of socket.frames) {
      combined.set(frame, offset)
      offset += frame.byteLength
    }
    expect(new TextDecoder().decode(combined)).toBe(input)
  })

  it('rejects disconnected, oversized, and backpressured sends before writing', () => {
    const disconnected = new FakeSocket()
    disconnected.readyState = 0
    expect(() => sendTerminalInput(disconnected, 'secret')).toThrow(/尚未连接/)
    expect(disconnected.frames).toHaveLength(0)

    const oversized = new FakeSocket()
    expect(() => sendTerminalInput(oversized, 'x'.repeat(MAX_TERMINAL_INPUT_BATCH_BYTES + 1))).toThrow(/256 KiB/)
    expect(oversized.frames).toHaveLength(0)

    const backpressured = new FakeSocket()
    backpressured.bufferedAmount = MAX_TERMINAL_INPUT_BATCH_BYTES
    expect(() => sendTerminalInput(backpressured, 'x')).toThrow(/繁忙/)
    expect(backpressured.frames).toHaveLength(0)
  })
})
