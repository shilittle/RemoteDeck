import { createHash } from 'node:crypto'
import type { CodexActionRequest, CodexStatus } from '../../protocol/command'
import type { TerminalSession } from '../../protocol/terminal'
import type { ProfileRepository } from '../hosts/profile-repository'
import type { SshConnectionManager } from '../ssh/connection-manager'
import type { TerminalService } from '../terminal/terminal-service'
import { inDirectory, remoteExec, shellQuote } from './remote-exec'

export const CODEX_INSTALL_COMMAND = 'curl -fsSL https://chatgpt.com/codex/install.sh | sh' as const
export const codexInstallPlan = {
  command: CODEX_INSTALL_COMMAND,
  sourceUrl: 'https://chatgpt.com/codex/install.sh' as const,
  impact: '从 OpenAI 官方地址下载稳定版安装脚本并执行；脚本会在当前远端用户环境安装或更新 Codex CLI。'
}

export class CodexService {
  readonly #profiles: ProfileRepository
  readonly #connections: SshConnectionManager
  readonly #terminals: TerminalService

  constructor(profiles: ProfileRepository, connections: SshConnectionManager, terminals: TerminalService) {
    this.#profiles = profiles
    this.#connections = connections
    this.#terminals = terminals
  }

  installPlan(): typeof codexInstallPlan { return codexInstallPlan }

  async probe(hostId: string): Promise<CodexStatus> {
    const profile = await this.#profiles.get(hostId)
    const client = this.#connections.getOnlineClient(hostId)
    const workspace = profile.workspace?.remotePath ?? '~'
    const located = await remoteExec(client, 'command -v codex 2>/dev/null || true', 5000, 4096)
    const installed = located.stdout.trim().length > 0
    const [version, login, tmux, rootHelp, loginHelp, resumeHelp, updateHelp, branch, dirty] = await Promise.all([
      installed ? safeExec(client, 'codex --version') : emptyResult(),
      installed ? safeExec(client, 'codex login status') : emptyResult(127),
      safeExec(client, 'command -v tmux 2>/dev/null'),
      installed ? safeExec(client, 'codex --help') : emptyResult(127),
      installed ? safeExec(client, 'codex login --help') : emptyResult(127),
      installed ? safeExec(client, 'codex resume --help') : emptyResult(127),
      installed ? safeExec(client, 'codex update --help') : emptyResult(127),
      safeExec(client, inDirectory('git branch --show-current', workspace)),
      safeExec(client, inDirectory('git status --porcelain', workspace))
    ])
    return {
      schemaVersion: 1,
      hostId,
      installed,
      version: installed && version.code === 0 ? firstLine(version.stdout || version.stderr) || null : null,
      login: !installed ? 'unknown' : login.code === 0 ? 'logged_in' : 'logged_out',
      tmuxInstalled: tmux.code === 0 && tmux.stdout.trim().length > 0,
      workspacePath: workspace,
      gitBranch: branch.code === 0 ? firstLine(branch.stdout) || null : null,
      gitDirty: branch.code === 0 ? dirty.stdout.trim().length > 0 : null,
      capabilities: {
        deviceAuth: loginHelp.code === 0 && outputOf(loginHelp).includes('--device-auth'),
        resume: outputOf(rootHelp).includes('resume') && resumeHelp.code === 0,
        resumeLast: outputOf(resumeHelp).includes('--last'),
        update: outputOf(rootHelp).includes('update') && updateHelp.code === 0
      },
      probedAt: new Date().toISOString()
    }
  }

  async action(request: CodexActionRequest): Promise<{ action: CodexActionRequest['action']; command: string; terminal: TerminalSession; tmuxSession: string | null }> {
    const profile = await this.#profiles.get(request.hostId)
    const status = await this.probe(request.hostId)
    const cwd = profile.workspace?.remotePath ?? '~'
    if (request.action !== 'install' && !status.installed) throw new Error('Codex CLI is not installed on this host')
    let command: string
    let tmuxSession: string | null = null
    switch (request.action) {
      case 'install':
        requireInstallerConfirmation(request.confirmed)
        command = CODEX_INSTALL_COMMAND
        break
      case 'login':
        command = `exec codex login${status.capabilities.deviceAuth ? ' --device-auth' : ''}`
        break
      case 'update':
        requireInstallerConfirmation(request.confirmed)
        if (status.capabilities.update) command = 'exec codex update'
        else command = CODEX_INSTALL_COMMAND
        break
      case 'resume':
        if (!status.capabilities.resume) throw new Error('The installed Codex version does not support resume')
        command = `exec codex resume${status.capabilities.resumeLast ? ' --last' : ''}`
        break
      case 'tmux':
      case 'reattach':
        if (!status.tmuxInstalled) throw new Error('tmux is not installed on this host')
        tmuxSession = sessionName(request.hostId, cwd)
        command = `exec tmux new-session -A -s ${shellQuote(tmuxSession)} -c ${shellPath(cwd)} codex`
        break
      case 'start':
        command = 'exec codex'
        break
    }
    const terminal = await this.#terminals.create({ hostId: request.hostId, cwd, cols: 120, rows: 34 })
    this.#terminals.write(terminal.id, `${request.action === 'tmux' || request.action === 'reattach' ? command : inDirectory(command, cwd)}\r`)
    return { action: request.action, command, terminal, tmuxSession }
  }
}

function requireInstallerConfirmation(confirmed: boolean): void {
  if (!confirmed) throw new Error('The official installer command requires explicit confirmation')
}

async function safeExec(client: ReturnType<SshConnectionManager['getOnlineClient']>, command: string): Promise<{ stdout: string; stderr: string; code: number | null; signal: string | null }> {
  try { return await remoteExec(client, command, 10_000, 256 * 1024) }
  catch (error) { return { stdout: '', stderr: error instanceof Error ? error.message : String(error), code: 255, signal: null } }
}

function emptyResult(code: number | null = null): Promise<{ stdout: string; stderr: string; code: number | null; signal: string | null }> {
  return Promise.resolve({ stdout: '', stderr: '', code, signal: null })
}

function firstLine(value: string): string { return value.trim().split(/\r?\n/, 1)[0] ?? '' }
function sessionName(hostId: string, cwd: string): string { return `remotedeck-${createHash('sha256').update(`${hostId}\0${cwd}`).digest('hex').slice(0, 12)}` }
function outputOf(result: { stdout: string; stderr: string }): string { return `${result.stdout}\n${result.stderr}` }
function shellPath(path: string): string { return path === '~' ? '"$HOME"' : path.startsWith('~/') ? `"$HOME"/${shellQuote(path.slice(2))}` : shellQuote(path) }
