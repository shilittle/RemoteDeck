import { useEffect, useMemo, useRef, useState } from 'react'
import { Bot, CircleAlert, Pencil, Play, Plus, RefreshCw, Save, ShieldCheck, Square, TerminalSquare, Trash2 } from 'lucide-react'
import { api, errorMessage } from '../api'
import { confirmationMatches, riskLabel } from '../risk'
import { agentActionNeedsConfirmation, buildAgentSessionRequest, isCurrentAgentStatus } from '../agent-contract'
import { canCommitCommandListing, isCommandListingCurrent } from '../command-listing'
import { useAppStore } from '../store'
import type {
  AgentAction,
  AgentCommandPlan,
  AgentKind,
  AgentSessionRequest,
  AgentStatus,
  CommandAnalysis,
  CommandDefinition,
  CommandDraft,
  CommandJob,
  CommandRunRequest
} from '../types'

const emptyDraft: CommandDraft = {
  name: '', description: '', group: '自定义', command: '', workingDirectory: '', risk: 'L1', requiresPty: false, requiresSudo: false, confirmationText: '', sortOrder: 100
}

interface PendingRun {
  name: string
  request: CommandRunRequest
  analysis: CommandAnalysis
}

interface PendingAgentAction {
  request: AgentSessionRequest
  plan: AgentCommandPlan
  targetAlias: string
}

export function CommandPanel(): React.JSX.Element {
  const agentProbeSequence = useRef(0)
  const agentPlanSequence = useRef(0)
  const commandLoadSequence = useRef(0)
  const selected = useAppStore((state) => state.hosts.find((host) => host.id === state.selectedHostId) ?? null)
  const settings = useAppStore((state) => state.settings)
  const setActivity = useAppStore((state) => state.setActivity)
  const jobs = useAppStore((state) => state.commandJobs)
  const setJobs = useAppStore((state) => state.setCommandJobs)
  const applyJob = useAppStore((state) => state.applyCommandJob)
  const [presets, setPresets] = useState<CommandDefinition[]>([])
  const [loadedHostId, setLoadedHostId] = useState<string | null>(null)
  const [draft, setDraft] = useState<CommandDraft>(emptyDraft)
  const [editing, setEditing] = useState(false)
  const [quickCommand, setQuickCommand] = useState('')
  const [quickDirectory, setQuickDirectory] = useState('')
  const [pending, setPending] = useState<PendingRun | null>(null)
  const [confirmation, setConfirmation] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')
  const [agent, setAgent] = useState<AgentKind>(settings.defaultAgent)
  const [agentStatus, setAgentStatus] = useState<AgentStatus | null>(null)
  const [agentSession, setAgentSession] = useState('')
  const [agentWorkspace, setAgentWorkspace] = useState('')
  const [pendingAgent, setPendingAgent] = useState<PendingAgentAction | null>(null)
  const [agentConfirmation, setAgentConfirmation] = useState('')
  const selectedHostIdRef = useRef<string | null>(selected?.id ?? null)
  selectedHostIdRef.current = selected?.id ?? null
  const hostJobs = useMemo(() => selected ? jobs.filter((job) => job.hostId === selected.id) : [], [jobs, selected])
  const visiblePresets = isCommandListingCurrent(loadedHostId, selected?.id ?? null) ? presets : []
  const grouped = useMemo(() => Object.entries(groupCommands(visiblePresets)), [visiblePresets])

  const load = async (): Promise<void> => {
    if (!selected) return
    const hostId = selected.id
    const generation = ++commandLoadSequence.current
    setBusy(true); setError('')
    try {
      const [nextPresets, nextJobs] = await Promise.all([api.listCommands(hostId), api.listCommandJobs(hostId)])
      if (!canCommitCommandListing(
        { hostId, generation },
        selectedHostIdRef.current,
        commandLoadSequence.current,
        nextPresets
      )) return
      setPresets(nextPresets)
      setLoadedHostId(hostId)
      setJobs(nextJobs)
    } catch (reason) {
      if (generation === commandLoadSequence.current && selectedHostIdRef.current === hostId) setError(errorMessage(reason))
    } finally {
      if (generation === commandLoadSequence.current && selectedHostIdRef.current === hostId) setBusy(false)
    }
  }

  const loadAgent = async (hostId: string, provider: AgentKind): Promise<void> => {
    const sequence = ++agentProbeSequence.current
    setAgentStatus(null)
    try {
      const next = await api.probeAgent(hostId, provider)
      if (sequence === agentProbeSequence.current && isCurrentAgentStatus(next, hostId, provider)) setAgentStatus(next)
    } catch {
      if (sequence === agentProbeSequence.current) setAgentStatus(null)
    }
  }

  useEffect(() => {
    commandLoadSequence.current += 1
    setPresets([])
    setLoadedHostId(null)
    setPending(null)
    setConfirmation('')
    setEditing(false)
    if (selected) void load()
  }, [selected?.id])
  useEffect(() => {
    agentProbeSequence.current += 1
    setAgentStatus(null)
    if (!selected) return
    const hostId = selected.id
    const timer = window.setTimeout(() => { void loadAgent(hostId, agent) }, 250)
    return () => {
      window.clearTimeout(timer)
      agentProbeSequence.current += 1
    }
  }, [agent, selected?.id])
  useEffect(() => {
    agentPlanSequence.current += 1
    setAgentSession('')
    setAgentWorkspace('')
    setPendingAgent(null)
    setAgentConfirmation('')
  }, [agent, selected?.id])
  useEffect(() => {
    if (agentSession && agentStatus && !agentStatus.resumableSessions.includes(agentSession)) setAgentSession('')
  }, [agentSession, agentStatus])

  const run = async (operation: () => Promise<void>): Promise<void> => {
    setBusy(true); setError(''); setMessage('')
    try { await operation() } catch (reason) { setError(errorMessage(reason)) } finally { setBusy(false) }
  }

  const savePreset = async (): Promise<void> => {
    if (!draft.name.trim() || !draft.command.trim()) { setError('名称和命令不能为空。'); return }
    const hostId = selected?.id ?? null
    await run(async () => {
      const saved = await api.saveCommand({ ...draft, hostId: draft.hostId ?? selected?.id ?? null })
      if (selectedHostIdRef.current !== hostId) return
      setPresets((current) => current.some((item) => item.id === saved.id) ? current.map((item) => item.id === saved.id ? saved : item) : [...current, saved])
      setEditing(false); setDraft(emptyDraft); setMessage('命令预设已保存。')
    })
  }

  const analyze = async (name: string, request: CommandRunRequest): Promise<void> => {
    const generation = commandLoadSequence.current
    await run(async () => {
      const analysis = await api.analyzeCommand(request)
      if (generation !== commandLoadSequence.current || selectedHostIdRef.current !== request.hostId) return
      setPending({ name, request, analysis }); setConfirmation('')
    })
  }

  const confirmRun = async (): Promise<void> => {
    if (!selected || !pending || pending.request.hostId !== selected.id || !confirmationMatches(pending.analysis, confirmation)) return
    await run(async () => {
      const backendConfirmation = pending.analysis.effectiveRisk === 'L1'
        ? pending.analysis.requiredConfirmation
        : confirmation || null
      const job = await api.runCommand({ ...pending.request, confirmation: backendConfirmation })
      applyJob(job); setPending(null); setConfirmation(''); setMessage('命令任务已创建。')
    })
  }

  const startAgent = async (request: AgentSessionRequest): Promise<void> => {
    await run(async () => {
      const result = await api.startAgentSession(request)
      window.dispatchEvent(new CustomEvent('remotedeck:terminal-created', { detail: result.terminal }))
      setActivity('terminal')
      setMessage(result.message)
      setPendingAgent(null)
      setAgentConfirmation('')
    })
  }

  const prepareAgent = async (action: AgentAction): Promise<void> => {
    if (!selected) return
    const request = buildAgentSessionRequest({
      hostId: selected.id,
      agent,
      action,
      workspace: agentWorkspace.trim() || selected.defaultWorkspace || null,
      sessionName: agentSession,
      confirmation: null
    })
    if (!agentActionNeedsConfirmation(action)) {
      await startAgent(request)
      return
    }
    const sequence = ++agentPlanSequence.current
    await run(async () => {
      const plan = await api.agentSessionPlan(request)
      if (sequence !== agentPlanSequence.current) return
      if (!plan.requiresConfirmation) throw new Error('后端未对安装或更新计划设置确认门禁。')
      setPendingAgent({ request, plan, targetAlias: selected.alias })
      setAgentConfirmation('')
    })
  }

  const confirmAgent = async (): Promise<void> => {
    if (!pendingAgent || agentConfirmation !== pendingAgent.targetAlias) return
    await startAgent({ ...pendingAgent.request, confirmation: agentConfirmation })
  }

  if (!selected) return <section className="empty-state"><TerminalSquare size={42} /><h1>选择一台主机</h1><p>命令风险判断和 AI Agent 会话都需要明确的目标主机。</p></section>
  const currentAgentStatus = isCurrentAgentStatus(agentStatus, selected.id, agent) ? agentStatus : null

  return (
    <section className="command-workspace">
      <header className="command-toolbar"><div><h1>命令与 AI Agent</h1><p>{selected.alias} · 后端强制执行 L0 / L1 / L2 风险门禁</p></div><div className="button-row wrap"><button disabled={busy} onClick={() => { void load(); void loadAgent(selected.id, agent) }}><RefreshCw size={14} />刷新</button><button className="primary" onClick={() => { setDraft({ ...emptyDraft, hostId: selected.id }); setEditing(true) }}><Plus size={14} />新建预设</button></div></header>
      <div className="command-boundary"><ShieldCheck size={15} /><span>界面风险标签仅用于说明；实际等级、确认文本和执行许可始终由 Rust 后端重新计算。</span></div>
      {error && <div className="notice error" role="alert">{error}<button onClick={() => setError('')}>关闭</button></div>}
      {message && <div className="notice success" role="status">{message}</div>}

      <section className="card quick-command"><div className="card-title"><TerminalSquare size={16} /><h2>一次性命令</h2></div><div className="quick-command-grid"><label><span>工作目录（可选）</span><input value={quickDirectory} onChange={(event) => setQuickDirectory(event.target.value)} placeholder="~/project" /></label><label className="wide"><span>命令</span><textarea value={quickCommand} onChange={(event) => setQuickCommand(event.target.value)} spellCheck={false} placeholder="git status" /></label></div><button className="primary" disabled={busy || !quickCommand.trim()} onClick={() => { void analyze('一次性命令', { hostId: selected.id, command: quickCommand, workingDirectory: quickDirectory || null }) }}><Play size={14} />分析并执行</button></section>

      {editing && <PresetEditor draft={draft} busy={busy} onChange={setDraft} onSave={() => { void savePreset() }} onCancel={() => setEditing(false)} />}

      <div className="command-columns">
        <div className="command-library">
          {grouped.length === 0 ? <section className="card empty-card"><h2>暂无命令预设</h2><p>创建预设后仍会在每次执行前重新分析风险。</p></section> : grouped.map(([group, items]) => <section className="command-group" key={group}><h2>{group}</h2><div className="command-grid">{items.map((preset) => <article className={`command-card risk-${preset.risk.toLowerCase()}`} key={preset.id}><div className="command-card-head"><span className="risk-badge">{preset.risk}</span><div><h3>{preset.name}</h3><p>{preset.description}</p></div></div><code>{preset.command}</code><div className="command-meta"><span>{riskLabel(preset.risk)}</span>{preset.requiresPty && <span>PTY</span>}{preset.requiresSudo && <span>sudo</span>}{preset.workingDirectory && <span>{preset.workingDirectory}</span>}</div><div className="button-row wrap"><button className="primary" disabled={busy} onClick={() => { void analyze(preset.name, { hostId: selected.id, commandId: preset.id }) }}><Play size={13} />执行</button><button disabled={busy || preset.builtin} onClick={() => { setDraft(definitionToDraft(preset)); setEditing(true) }}><Pencil size={13} />编辑</button><button className="danger" disabled={busy || preset.builtin} onClick={() => { if (window.confirm(`删除预设“${preset.name}”？`)) { const hostId = selected.id; void run(async () => { await api.deleteCommand(preset.id, hostId); if (selectedHostIdRef.current !== hostId) return; setPresets((current) => current.filter((item) => item.id !== preset.id)); setMessage('预设已删除。') }) } }}><Trash2 size={13} />删除</button></div></article>)}</div></section>)}
        </div>
        <AgentCard selectedAlias={selected.alias} agent={agent} setAgent={setAgent} status={currentAgentStatus} workspace={agentWorkspace} setWorkspace={setAgentWorkspace} sessionName={agentSession} setSessionName={setAgentSession} busy={busy} onAction={(action) => { void prepareAgent(action) }} />
      </div>

      <CommandJobs jobs={hostJobs} onApply={applyJob} onError={setError} />
      {pending && <ConfirmationDialog pending={pending} value={confirmation} busy={busy} onChange={setConfirmation} onClose={() => setPending(null)} onConfirm={() => { void confirmRun() }} />}
      {pendingAgent && <AgentPlanDialog pending={pendingAgent} value={agentConfirmation} busy={busy} onChange={setAgentConfirmation} onClose={() => { setPendingAgent(null); setAgentConfirmation('') }} onConfirm={() => { void confirmAgent() }} />}
    </section>
  )
}

function PresetEditor({ draft, busy, onChange, onSave, onCancel }: { draft: CommandDraft; busy: boolean; onChange: (draft: CommandDraft) => void; onSave: () => void; onCancel: () => void }): React.JSX.Element {
  return <section className="card preset-editor"><div className="card-title"><Save size={16} /><h2>{draft.id ? '编辑命令预设' : '新建命令预设'}</h2></div><div className="preset-form"><label><span>名称</span><input value={draft.name} onChange={(event) => onChange({ ...draft, name: event.target.value })} /></label><label><span>分组</span><input value={draft.group} onChange={(event) => onChange({ ...draft, group: event.target.value })} /></label><label className="wide"><span>说明</span><input value={draft.description} onChange={(event) => onChange({ ...draft, description: event.target.value })} /></label><label className="wide"><span>命令</span><textarea value={draft.command} onChange={(event) => onChange({ ...draft, command: event.target.value })} spellCheck={false} /></label><label><span>工作目录</span><input value={draft.workingDirectory ?? ''} onChange={(event) => onChange({ ...draft, workingDirectory: event.target.value })} /></label><label><span>声明风险</span><select value={draft.risk} onChange={(event) => onChange({ ...draft, risk: event.target.value as CommandDraft['risk'] })}><option value="L0">L0 · 只读</option><option value="L1">L1 · 有修改</option><option value="L2">L2 · 高风险</option></select></label><label className="wide"><span>L2 确认文本（可选）</span><input value={draft.confirmationText ?? ''} onChange={(event) => onChange({ ...draft, confirmationText: event.target.value })} /></label><label className="toggle"><input type="checkbox" checked={draft.requiresPty} onChange={(event) => onChange({ ...draft, requiresPty: event.target.checked })} /><span>需要 PTY</span></label><label className="toggle"><input type="checkbox" checked={draft.requiresSudo} onChange={(event) => onChange({ ...draft, requiresSudo: event.target.checked })} /><span>需要 sudo</span></label></div><div className="button-row"><button className="primary" disabled={busy || !draft.name.trim() || !draft.command.trim()} onClick={onSave}>保存</button><button disabled={busy} onClick={onCancel}>取消</button></div></section>
}

function AgentCard({ selectedAlias, agent, setAgent, status, workspace, setWorkspace, sessionName, setSessionName, busy, onAction }: { selectedAlias: string; agent: AgentKind; setAgent: (agent: AgentKind) => void; status: AgentStatus | null; workspace: string; setWorkspace: (value: string) => void; sessionName: string; setSessionName: (value: string) => void; busy: boolean; onAction: (action: AgentAction) => void }): React.JSX.Element {
  return <aside className="card agent-card"><div className="agent-title"><Bot size={19} /><div><h2>AI Agent</h2><p>在标准 SSH PTY / tmux 会话中运行，不绑定单一厂商。</p></div></div><label><span>Agent</span><select value={agent} onChange={(event) => setAgent(event.target.value as AgentKind)}><option value="codex">Codex CLI</option><option value="claude">Claude Code</option><option value="gemini">Gemini CLI</option><option value="opencode">OpenCode</option></select></label><dl><dt>目标</dt><dd>{selectedAlias}</dd><dt>安装</dt><dd>{status?.installed ? status.version ?? '已安装' : '未检测到'}</dd><dt>登录</dt><dd>{status?.authenticated === true ? '已登录' : status?.authenticated === false ? '未登录' : '由 CLI 确认'}</dd><dt>tmux</dt><dd>{status?.tmuxAvailable ? '可用' : '不可用'}</dd></dl>{status?.installHint && <p className="agent-hint">{status.installHint}</p>}<label><span>工作区</span><input value={workspace} onChange={(event) => setWorkspace(event.target.value)} placeholder="~/project" /></label><label><span>恢复 RemoteDeck tmux 会话（可选）</span><select value={sessionName} onChange={(event) => setSessionName(event.target.value)}><option value="">稳定会话（自动附加或新建）</option>{status?.resumableSessions.map((session) => <option key={session} value={session}>{session}</option>)}</select></label><div className="agent-actions"><button disabled={busy} onClick={() => onAction('install')}>安装</button><button disabled={busy} onClick={() => onAction('login')}>登录</button><button className="primary" disabled={busy || !status?.installed || !status.tmuxAvailable} onClick={() => onAction('start')}>启动</button><button disabled={busy || !status?.installed || !status.tmuxAvailable} onClick={() => onAction('resume')}>恢复</button><button disabled={busy || !status?.installed} onClick={() => onAction('update')}>更新</button></div></aside>
}

function AgentPlanDialog({ pending, value, busy, onChange, onClose, onConfirm }: { pending: PendingAgentAction; value: string; busy: boolean; onChange: (value: string) => void; onClose: () => void; onConfirm: () => void }): React.JSX.Element {
  const allowed = value === pending.targetAlias
  return <div className="command-modal-backdrop" role="presentation"><div className="command-modal agent-plan-modal" role="dialog" aria-modal="true" aria-labelledby="agent-plan-title"><h2 id="agent-plan-title">确认 {pending.plan.displayName} {pending.plan.action === 'install' ? '安装' : '更新'}</h2><p>目标主机：<strong>{pending.targetAlias}</strong> · 工作区：<strong>{pending.request.workspace ?? '~'}</strong></p><p>{pending.plan.notice}</p><ol className="agent-plan-commands">{pending.plan.commands.map((command, index) => <li key={`${command.command}-${String(index)}`}><span>{command.purpose}</span><code>{command.command}</code></li>)}</ol>{pending.plan.sourceUrl && <p>官方来源：<a href={pending.plan.sourceUrl} target="_blank" rel="noreferrer">{pending.plan.sourceUrl}</a></p>}<label>输入主机别名 <strong>{pending.targetAlias}</strong> 继续<input autoFocus value={value} onChange={(event) => onChange(event.target.value)} /></label><div className="button-row"><button className="danger" disabled={busy || !allowed} onClick={onConfirm}>确认执行</button><button disabled={busy} onClick={onClose}>取消</button></div></div></div>
}

function CommandJobs({ jobs, onApply, onError }: { jobs: CommandJob[]; onApply: (job: CommandJob) => void; onError: (message: string) => void }): React.JSX.Element {
  return <section className="card command-jobs">
    <h2>命令任务</h2>
    {jobs.length === 0 ? <p className="muted">暂无命令任务。</p> : jobs.toReversed().slice(0, 30).map((job) => <article key={job.id}>
      <div>
        <span className={`job-state state-${job.state}`}>{job.state}</span>
        <strong>{job.name}</strong>
        <span>{job.risk}</span>
        {(job.state === 'running' || job.state === 'queued') && <button onClick={() => { void api.cancelCommand(job.id).then(onApply).catch((reason: unknown) => onError(errorMessage(reason))) }}><Square size={12} />取消</button>}
        {job.state === 'cancelling' && <button disabled><Square size={12} />取消中</button>}
      </div>
      <code>{job.command}</code>
      {job.stdout && <pre>{job.stdout}</pre>}
      {job.stderr && <pre className="stderr">{job.stderr}</pre>}
      {job.error && <p>{job.error}</p>}
    </article>)}
  </section>
}

function ConfirmationDialog({ pending, value, busy, onChange, onClose, onConfirm }: { pending: PendingRun; value: string; busy: boolean; onChange: (value: string) => void; onClose: () => void; onConfirm: () => void }): React.JSX.Element {
  const { analysis } = pending
  const allowed = confirmationMatches(analysis, value)
  return <div className="command-modal-backdrop" role="presentation"><div className="command-modal" role="dialog" aria-modal="true" aria-labelledby="command-confirm-title"><h2 id="command-confirm-title">确认 {analysis.effectiveRisk} 命令</h2><p>预设：<strong>{pending.name}</strong> · 目标：<strong>{analysis.targetAlias}</strong> · 目录：<strong>{analysis.workingDirectory ?? '~'}</strong></p><pre>{analysis.displayCommand}</pre>{analysis.reasons.length > 0 && <ul>{analysis.reasons.map((reason) => <li key={reason}>{reason}</li>)}</ul>}{analysis.effectiveRisk === 'L2' && <label>输入 <strong>{analysis.requiredConfirmation ?? '确认文本'}</strong> 继续<input autoFocus value={value} onChange={(event) => onChange(event.target.value)} /></label>}{analysis.effectiveRisk === 'L1' && <p className="risk-warning"><CircleAlert size={14} />此命令会修改远端状态，请再次确认。</p>}<div className="button-row"><button className={analysis.effectiveRisk === 'L2' ? 'danger' : 'primary'} disabled={busy || !allowed} onClick={onConfirm}>确认执行</button><button disabled={busy} onClick={onClose}>取消</button></div></div></div>
}

function definitionToDraft(preset: CommandDefinition): CommandDraft {
  return { id: preset.id, hostId: preset.hostId, name: preset.name, description: preset.description, group: preset.group, command: preset.command, workingDirectory: preset.workingDirectory, risk: preset.risk, requiresPty: preset.requiresPty, requiresSudo: preset.requiresSudo, confirmationText: preset.confirmationText, sortOrder: preset.sortOrder }
}

function groupCommands(presets: CommandDefinition[]): Record<string, CommandDefinition[]> {
  return presets.reduce<Record<string, CommandDefinition[]>>((groups, preset) => {
    const key = preset.group || '其他'
    return { ...groups, [key]: [...(groups[key] ?? []), preset] }
  }, {})
}
