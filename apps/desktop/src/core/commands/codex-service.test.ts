import { Duplex, PassThrough } from 'node:stream'
import { describe, expect, it } from 'vitest'
import { CODEX_INSTALL_COMMAND, CodexService } from './codex-service'

const hostId = '019f92f0-b87c-7c74-a668-54d5b18fb487'

class ResponseChannel extends Duplex {
  readonly stderr = new PassThrough()
  constructor(stdout: string, stderr: string, code: number) {
    super()
    setImmediate(() => { if (stdout) this.push(stdout); if (stderr) this.stderr.write(stderr); this.push(null); this.stderr.end(); this.emit('exit', code); this.emit('close') })
  }
  _read(): void { /* response scheduled in constructor */ }
  _write(_chunk: Buffer, _encoding: BufferEncoding, callback: (error?: Error | null) => void): void { callback() }
}

function harness(mode: 'absent' | 'logged_out' | 'logged_in') {
  const commands: string[] = []
  const writes: string[] = []
  const client = {
    exec: (command: string, callback: (error: Error | undefined, channel: ResponseChannel) => void) => {
      commands.push(command)
      let result: [string, string, number] = ['', '', 0]
      if (command === 'command -v codex 2>/dev/null || true') result = mode === 'absent' ? ['', '', 0] : ['/usr/local/bin/codex\n', '', 0]
      else if (command === 'codex --version') result = ['codex-cli 1.2.3\n', '', 0]
      else if (command === 'codex login status') result = mode === 'logged_in' ? ['Logged in using ChatGPT\n', '', 0] : ['', 'Not logged in\n', 1]
      else if (command.includes('command -v tmux')) result = ['/usr/bin/tmux\n', '', 0]
      else if (command === 'codex --help') result = ['Commands: login resume update\n', '', 0]
      else if (command === 'codex login --help') result = ['Usage: codex login --device-auth\n', '', 0]
      else if (command === 'codex resume --help') result = ['Usage: codex resume --last --all\n', '', 0]
      else if (command === 'codex update --help') result = ['Usage: codex update\n', '', 0]
      else if (command.includes('git branch --show-current')) result = ['main\n', '', 0]
      else if (command.includes('git status --porcelain')) result = [' M src.ts\n', '', 0]
      callback(undefined, new ResponseChannel(...result))
    }
  }
  const profiles = { get: () => Promise.resolve({ host: { id: hostId, alias: 'worker' }, workspace: { remotePath: '/srv/repo' } }) }
  const terminals = {
    create: () => Promise.resolve({ id: '30000000-0000-4000-8000-000000000001', hostId, hostAlias: 'worker', cwd: '/srv/repo', cols: 120, rows: 34, generation: 1, state: 'online', openedAt: new Date().toISOString() }),
    write: (_id: string, data: string) => { writes.push(data) }
  }
  return { service: new CodexService(profiles as never, { getOnlineClient: () => client } as never, terminals as never), commands, writes }
}

describe('Codex remote integration', () => {
  it.each([
    ['absent', false, 'unknown'],
    ['logged_out', true, 'logged_out'],
    ['logged_in', true, 'logged_in']
  ] as const)('probes %s without reading credential files', async (mode, installed, login) => {
    const { service, commands } = harness(mode)
    await expect(service.probe(hostId)).resolves.toMatchObject({ installed, login })
    expect(commands.join('\n')).not.toMatch(/auth\.json|CODEX_ACCESS_TOKEN|OPENAI_API_KEY/)
  })

  it('uses runtime-detected device auth, resume, self-update, and stable tmux association', async () => {
    const { service, writes } = harness('logged_in')
    await service.action({ hostId, action: 'login', confirmed: false })
    await service.action({ hostId, action: 'resume', confirmed: false })
    await service.action({ hostId, action: 'update', confirmed: true })
    const firstTmux = await service.action({ hostId, action: 'tmux', confirmed: false })
    const secondTmux = await service.action({ hostId, action: 'reattach', confirmed: false })
    expect(writes.join('\n')).toContain('codex login --device-auth')
    expect(writes.join('\n')).toContain('codex resume --last')
    expect(writes.join('\n')).toContain('codex update')
    expect(firstTmux.tmuxSession).toMatch(/^remotedeck-[a-f0-9]{12}$/)
    expect(secondTmux.tmuxSession).toBe(firstTmux.tmuxSession)
    expect(writes.join('\n')).not.toContain('--dangerously-bypass-approvals-and-sandbox')
  })

  it('requires confirmation for the official standalone installer', async () => {
    const { service, writes } = harness('absent')
    await expect(service.action({ hostId, action: 'install', confirmed: false })).rejects.toThrow(/confirmation/)
    await service.action({ hostId, action: 'install', confirmed: true })
    expect(writes.at(-1)).toContain(CODEX_INSTALL_COMMAND)
  })
})
