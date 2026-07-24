import ssh2 from 'ssh2'

export type Ed25519KeyPair = { private: string; public: string }
export type Ed25519KeyOptions =
  | { comment: string }
  | { comment: string; passphrase: string; cipher: 'aes256-ctr'; rounds: number }

const maximumAttempts = 8

export function generateUsableEd25519KeyPair(options: Ed25519KeyOptions, generator?: () => Ed25519KeyPair): Ed25519KeyPair {
  for (let attempt = 0; attempt < maximumAttempts; attempt += 1) {
    const pair = generator?.() ?? ('passphrase' in options
      ? ssh2.utils.generateKeyPairSync('ed25519', options)
      : ssh2.utils.generateKeyPairSync('ed25519', options))
    const privateKey = ssh2.utils.parseKey(pair.private, 'passphrase' in options ? options.passphrase : undefined)
    const publicKey = ssh2.utils.parseKey(pair.public)
    if (!(privateKey instanceof Error) && !(publicKey instanceof Error) && privateKey.type === 'ssh-ed25519' && publicKey.type === 'ssh-ed25519' && privateKey.getPublicSSH().equals(publicKey.getPublicSSH())) return pair
  }
  throw new Error(`Unable to generate a valid Ed25519 OpenSSH key pair after ${String(maximumAttempts)} attempts`)
}
