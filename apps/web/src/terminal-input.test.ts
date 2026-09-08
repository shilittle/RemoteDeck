import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  MAX_TERMINAL_INPUT_BATCH_BYTES,
  MAX_TERMINAL_INPUT_FRAME_BYTES,
  sendTerminalInput,
  terminalWebSocketUrl
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
  afterEach(() => vi.unstubAllGlobals())

  it('uses bounded binary frames and never serializes terminal input into a request body', () => {
    expect(MAX_TERMINAL_INPUT_FRAME_BYTES).toBe(64 * 1024)
    expect(MAX_TERMINAL_INPUT_BATCH_BYTES).toBe(256 * 1024)
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

  it('accepts a same-origin ws endpoint after normalizing its transport protocol', () => {
    vi.stubGlobal('window', { location: { href: 'http://127.0.0.1:1420/', origin: 'http://127.0.0.1:1420', protocol: 'http:' } })
    expect(terminalWebSocketUrl({ url: 'ws://127.0.0.1:1420/api/v1/terminal/input?ticket=one', generation: 3 })).toBe('ws://127.0.0.1:1420/api/v1/terminal/input?ticket=one')
    expect(() => terminalWebSocketUrl({ url: 'ws://127.0.0.1:1421/api/v1/terminal/input?ticket=two', generation: 3 })).toThrow(/跨域/)
  })
})
