import { describe, expect, it } from 'vitest'
import { validateHostDraft, validateTunnelDraft } from './validation'

const host = {
  alias: 'lab-gpu',
  hostname: '10.0.0.2',
  port: 22,
  username: 'researcher',
  groups: []
}

describe('host validation', () => {
  it('accepts a normal OpenSSH profile', () => expect(validateHostDraft(host)).toBeNull())
  it('rejects option injection', () => expect(validateHostDraft({ ...host, hostname: '-oProxyCommand=bad' })).not.toBeNull())
})

describe('tunnel validation', () => {
  it('rejects invalid ports', () => expect(validateTunnelDraft({
    hostId: 'id', name: 'Jupyter', direction: 'local', bindAddress: '127.0.0.1', sourcePort: 0,
    targetHost: '127.0.0.1', targetPort: 8888
  })).not.toBeNull())
})
