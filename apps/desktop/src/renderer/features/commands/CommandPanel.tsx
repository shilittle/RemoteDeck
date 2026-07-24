import { useEffect, useMemo, useState } from 'react'
import { Bot, CirclePlay, Copy, Pencil, Plus, RefreshCw, Save, Square, Trash2, X } from 'lucide-react'
import type { CodexStatus, CommandAnalysis, CommandDefinition, CommandJob, CommandPresetInput } from '../../../protocol/command'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

const blankPreset: CommandPresetInput = {
  name: '', description: '', group: '', command: '', risk: 'L1', requiresPty: false, requiresSudo: false, sortOrder: 100
}

export function CommandPanel(): React.JSX.Element {
  const hosts = useHostStore((state) => state.items)
  const selectedId = useHostStore((state) => state.selectedId)
  const host = hosts.find((item) => item.host.id === selectedId)
  const setActivity = useAppStore((state) => state.setActivity)
  const [presets, setPresets] = useState<CommandDefinition[]>([])
  const [jobs, setJobs] = useState<CommandJob[]>([])
  const [editor, setEditor] = useState<{ id?: string; value: CommandPresetInput } | null>(null)
  const [pending, setPending] = useState<{ preset: CommandDefinition; analysis: CommandAnalysis } | null>(null)
  const [confirmation, setConfirmation] = useState('')
  const [codex, setCodex] = useState<CodexStatus | null>(null)
  const [installPlan, setInstallPlan] = useState<{ command: string; sourceUrl: string; impact: string } | null>(null)
  const [installerConfirmed, setInstallerConfirmed] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')

  async function load(): Promise<void> {
    if (!host) { setPresets([]); setCodex(null); return }
    setBusy(true); setError('')
    try {
      const [items, currentJobs, plan] = await Promise.all([window.remoteDeck.commands.list(host.host.id), window.remoteDeck.commands.jobs(), window.remoteDeck.codex.installPlan()])
      setPresets(items); setJobs(currentJobs.filter((item) => item.hostId === host.host.id)); setInstallPlan(plan)
      if (host.state === 'online') setCodex(await window.remoteDeck.codex.probe(host.host.id))
      else setCodex(null)
    } catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  useEffect(() => { void load() }, [selectedId, host?.state])
  useEffect(() => window.remoteDeck.commands.onEvent((event) => {
    if (event.type === 'job') {
      if (event.job.hostId !== useHostStore.getState().selectedId) return
      setJobs((current) => current.some((item) => item.id === event.job.id) ? current.map((item) => item.id === event.job.id ? event.job : item) : [event.job, ...current])
      return
    }
    setJobs((current) => current.map((item) => item.id === event.jobId ? { ...item, output: `${item.output}${event.stream === 'stderr' ? '[stderr] ' : ''}${event.data}`.slice(-2 * 1024 * 1024) } : item))
  }), [])

  const grouped = useMemo(() => {
    const map = new Map<string, CommandDefinition[]>()
    for (const preset of presets) map.set(preset.group || '未分组', [...(map.get(preset.group || '未分组') ?? []), preset])
    return [...map.entries()]
  }, [presets])

  async function saveEditor(): Promise<void> {
    if (!editor || !host) return
    setBusy(true); setError('')
    try {
      if (editor.id) await window.remoteDeck.commands.update({ id: editor.id, patch: editor.value })
      else await window.remoteDeck.commands.create(editor.value)
      setEditor(null); await load()
    } catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  async function copyPreset(preset: CommandDefinition): Promise<void> {
    if (!host) return
    const { schemaVersion: _schema, id: _id, createdAt: _created, updatedAt: _updated, builtin: _builtin, available: _available, unavailableReason: _reason, ...input } = preset
    void [_schema, _id, _created, _updated, _builtin, _available, _reason]
    setBusy(true)
    try { await window.remoteDeck.commands.create({ ...input, name: `${preset.name}（副本）` }); await load() }
    catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  async function removePreset(id: string): Promise<void> {
    setBusy(true); setError('')
    try { await window.remoteDeck.commands.delete(id); await load() }
    catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  async function prepareRun(preset: CommandDefinition): Promise<void> {
    if (!host) return
    setError(''); setMessage('')
    try {
      const analysis = await window.remoteDeck.commands.analyze(host.host.id, preset.id)
      if (analysis.effectiveRisk === 'L0') { await execute(preset, analysis, false, ''); return }
      setConfirmation(''); setPending({ preset, analysis })
    } catch (reason) { setError(messageOf(reason)) }
  }

  async function execute(preset: CommandDefinition, analysis: CommandAnalysis, confirmed: boolean, confirmationInput: string): Promise<void> {
    if (!host) return
    setBusy(true); setError('')
    try {
      const job = await window.remoteDeck.commands.run({ hostId: host.host.id, presetId: preset.id, confirmed, confirmationInput })
      setJobs((current) => current.some((item) => item.id === job.id) ? current : [job, ...current])
      setPending(null)
      if (job.terminalSessionId) setActivity('terminal')
      else setMessage(`已启动“${preset.name}”任务。`)
    } catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  async function codexAction(action: 'install' | 'login' | 'update' | 'start' | 'resume' | 'tmux' | 'reattach'): Promise<void> {
    if (!host) return
    if ((action === 'install' || action === 'update') && !installerConfirmed) { setError('请先核对并确认官方安装或更新命令、来源与影响。'); return }
    setBusy(true); setError(''); setMessage('')
    try {
      const result = await window.remoteDeck.codex.action({ hostId: host.host.id, action, confirmed: installerConfirmed })
      setMessage(`已在真实 PTY 中启动：${result.command}`)
      setActivity('terminal')
    } catch (reason) { setError(messageOf(reason)) }
    finally { setBusy(false) }
  }

  if (!host) return <section className="command-state"><Bot size={36} /><h1>选择一台主机</h1><p>命令预设与 Codex 工作区按当前主机运行。</p></section>
  return (
    <section className="command-workspace">
      <div className="command-toolbar"><div><h1>命令与 Codex</h1><p>{host.host.alias} · {host.workspace?.remotePath ?? '~'} · {host.state}</p></div><div className="button-row"><button disabled={busy} onClick={() => { void load() }}><RefreshCw size={14} />刷新</button><button disabled={busy} onClick={() => setEditor({ value: { ...blankPreset, hostId: host.host.id } })}><Plus size={14} />新建预设</button></div></div>
      <div className="command-boundary">预设按钮由主进程执行风险分级与确认策略。自由 SSH 终端输入无法可靠拦截，因此不受此策略保护；请始终检查终端中即将执行的命令。</div>
      {error && <div className="command-error" role="alert">{error}</div>}{message && <div className="command-message">{message}</div>}
      {editor && <PresetEditor editor={editor} hostId={host.host.id} busy={busy} onChange={setEditor} onSave={() => { void saveEditor() }} onClose={() => setEditor(null)} />}
      <div className="command-columns">
        <div className="command-library">
          {grouped.map(([group, items]) => <section className="command-group" key={group}><h2>{group}</h2><div className="command-grid">{items.map((preset) => <article className={`command-card risk-${preset.risk.toLowerCase()}`} key={preset.id}><div className="command-card-head"><span className="risk-badge">{preset.risk}</span><div><h3>{preset.name}</h3><p>{preset.description || '无说明'}</p></div></div><code>{preset.command}</code><div className="command-meta"><span>{preset.builtin ? '内置' : preset.hostId ? '当前主机' : '全局'}</span><span>顺序 {preset.sortOrder}</span>{preset.requiresPty && <span>PTY</span>}{preset.requiresSudo && <span>sudo</span>}</div>{!preset.available && <p className="command-unavailable">{preset.unavailableReason}</p>}<div className="button-row"><button className="primary" disabled={busy || host.state !== 'online' || !preset.available} onClick={() => { void prepareRun(preset) }}><CirclePlay size={13} />运行</button><button disabled={busy} title="复制" onClick={() => { void copyPreset(preset) }}><Copy size={13} /></button>{!preset.builtin && <><button disabled={busy} title="编辑" onClick={() => setEditor({ id: preset.id, value: toInput(preset) })}><Pencil size={13} /></button><button className="danger" disabled={busy} title="删除" onClick={() => { void removePreset(preset.id) }}><Trash2 size={13} /></button></>}</div></article>)}</div></section>)}
        </div>
        <aside className="codex-card card"><div className="codex-title"><Bot size={20} /><div><h2>OpenAI Codex CLI</h2><p>只启动官方稳定 CLI；不解析 TUI，不读取凭据文件。</p></div></div>{host.state !== 'online' ? <p className="muted">连接主机后可探测 Codex。</p> : codex ? <><dl><dt>安装</dt><dd>{codex.installed ? codex.version : '未安装'}</dd><dt>登录</dt><dd>{loginLabel(codex.login)}</dd><dt>tmux</dt><dd>{codex.tmuxInstalled ? '可用' : '未检测到'}</dd><dt>工作区</dt><dd>{codex.workspacePath}</dd><dt>Git</dt><dd>{codex.gitBranch ? `${codex.gitBranch}${codex.gitDirty ? ' · 有改动' : ' · 干净'}` : '非 Git 工作区'}</dd></dl>{installPlan && <div className="installer-plan"><strong>官方安装 / 回退更新命令</strong><code>{installPlan.command}</code><a href={installPlan.sourceUrl} onClick={(event) => { event.preventDefault(); void window.remoteDeck.app.openExternal(installPlan.sourceUrl) }}>{installPlan.sourceUrl}</a><p>{installPlan.impact}</p><label><input type="checkbox" checked={installerConfirmed} onChange={(event) => setInstallerConfirmed(event.target.checked)} />我已核对来源、完整命令与影响</label></div>}<div className="codex-actions">{!codex.installed ? <button className="primary" disabled={!installerConfirmed || busy} onClick={() => { void codexAction('install') }}>安装 Codex</button> : <><button disabled={busy} onClick={() => { void codexAction('login') }}>{codex.login === 'logged_in' ? '重新登录' : '设备码登录'}</button><button disabled={busy || codex.login !== 'logged_in'} onClick={() => { void codexAction('start') }}>普通 Codex 终端</button><button disabled={busy || codex.login !== 'logged_in' || !codex.capabilities.resume} onClick={() => { void codexAction('resume') }}>{codex.capabilities.resumeLast ? '恢复最近会话' : '选择会话恢复'}</button><button disabled={busy || codex.login !== 'logged_in' || !codex.tmuxInstalled} onClick={() => { void codexAction('tmux') }}>启动持久 tmux</button><button disabled={busy || codex.login !== 'logged_in' || !codex.tmuxInstalled} onClick={() => { void codexAction('reattach') }}>重新附加 tmux</button><button disabled={busy || (!codex.capabilities.update && !installerConfirmed)} onClick={() => { void codexAction('update') }}>更新 Codex</button></>}</div></> : <p className="muted">正在探测……</p>}</aside>
      </div>
      <section className="command-jobs card"><h2>任务输出</h2>{!jobs.length ? <p className="muted">尚无命令任务。</p> : jobs.map((job) => <article key={job.id}><div><strong>{job.name}</strong><span className={`job-state state-${job.state}`}>{job.state}</span>{job.state === 'running' && <button onClick={() => { void window.remoteDeck.commands.cancel(job.id) }}><Square size={12} />取消</button>}</div><code>{job.command}</code><pre>{job.output || (job.state === 'running' ? '等待输出……' : job.error ?? '命令未产生输出')}</pre>{job.error && <p>{job.error}</p>}</article>)}</section>
      {pending && <ConfirmationDialog pending={pending} value={confirmation} busy={busy} onChange={setConfirmation} onClose={() => setPending(null)} onConfirm={() => { void execute(pending.preset, pending.analysis, true, confirmation) }} />}
    </section>
  )
}

function PresetEditor({ editor, hostId, busy, onChange, onSave, onClose }: { editor: { id?: string; value: CommandPresetInput }; hostId: string; busy: boolean; onChange: (value: { id?: string; value: CommandPresetInput }) => void; onSave: () => void; onClose: () => void }): React.JSX.Element {
  const value = editor.value
  const change = (patch: Partial<CommandPresetInput>): void => onChange({ ...editor, value: { ...value, ...patch } })
  return <section className="preset-editor card"><div className="card-title"><h2>{editor.id ? '编辑命令预设' : '新建命令预设'}</h2><button onClick={onClose}><X size={14} /></button></div><div className="preset-form"><label><span>名称</span><input value={value.name} onChange={(event) => change({ name: event.target.value })} /></label><label><span>分组</span><input value={value.group} onChange={(event) => change({ group: event.target.value })} /></label><label className="wide"><span>说明</span><input value={value.description} onChange={(event) => change({ description: event.target.value })} /></label><label className="wide"><span>命令</span><textarea value={value.command} onChange={(event) => change({ command: event.target.value })} /></label><label><span>工作目录（留空使用主机关联工作区）</span><input value={value.workingDirectory ?? ''} onChange={(event) => change({ workingDirectory: event.target.value || undefined })} /></label><label><span>风险</span><select value={value.risk} onChange={(event) => change({ risk: event.target.value as CommandPresetInput['risk'] })}><option value="L0">L0 只读</option><option value="L1">L1 状态变更</option><option value="L2">L2 高风险</option></select></label><label><span>排序</span><input type="number" value={value.sortOrder} onChange={(event) => change({ sortOrder: Number(event.target.value) })} /></label><label><span>L2 指定确认文字（留空使用主机别名）</span><input value={value.confirmationText ?? ''} onChange={(event) => change({ confirmationText: event.target.value || undefined })} /></label><label className="check"><input type="checkbox" checked={!value.hostId} onChange={(event) => change({ hostId: event.target.checked ? undefined : hostId })} />全局预设</label><label className="check"><input type="checkbox" checked={value.requiresPty} onChange={(event) => change({ requiresPty: event.target.checked })} />需要真实 PTY</label><label className="check"><input type="checkbox" checked={value.requiresSudo} onChange={(event) => change({ requiresSudo: event.target.checked })} />通过 sudo 执行</label></div><div className="button-row"><button className="primary" disabled={busy || !value.name.trim() || !value.command.trim()} onClick={onSave}><Save size={14} />保存</button><button disabled={busy} onClick={onClose}>取消</button></div></section>
}

function ConfirmationDialog({ pending, value, busy, onChange, onClose, onConfirm }: { pending: { preset: CommandDefinition; analysis: CommandAnalysis }; value: string; busy: boolean; onChange: (value: string) => void; onClose: () => void; onConfirm: () => void }): React.JSX.Element {
  const { preset, analysis } = pending
  const allowed = analysis.effectiveRisk === 'L1' || value === analysis.requiredConfirmation
  return <div className="command-modal-backdrop" role="presentation"><div className="command-modal" role="dialog" aria-modal="true" aria-labelledby="command-confirm-title"><h2 id="command-confirm-title">确认 {analysis.effectiveRisk} 命令</h2><p>目标：<strong>{analysis.targetAlias}</strong> · 目录：<strong>{analysis.workingDirectory ?? '~'}</strong></p><pre>{analysis.displayCommand ?? preset.command}</pre>{analysis.reasons.length > 0 && <ul>{analysis.reasons.map((reason) => <li key={reason}>{reason}</li>)}</ul>}{analysis.effectiveRisk === 'L2' && <label>输入 <strong>{analysis.requiredConfirmation}</strong> 继续<input autoFocus value={value} onChange={(event) => onChange(event.target.value)} /></label>}<div className="button-row"><button className={analysis.effectiveRisk === 'L2' ? 'danger' : 'primary'} disabled={busy || !allowed} onClick={onConfirm}>确认执行</button><button disabled={busy} onClick={onClose}>取消</button></div></div></div>
}

function toInput(preset: CommandDefinition): CommandPresetInput {
  return { name: preset.name, description: preset.description, group: preset.group, command: preset.command, risk: preset.risk, requiresPty: preset.requiresPty, requiresSudo: preset.requiresSudo, sortOrder: preset.sortOrder, ...(preset.hostId ? { hostId: preset.hostId } : {}), ...(preset.workingDirectory ? { workingDirectory: preset.workingDirectory } : {}), ...(preset.confirmationText ? { confirmationText: preset.confirmationText } : {}) }
}
function loginLabel(value: CodexStatus['login']): string { return value === 'logged_in' ? '已登录' : value === 'logged_out' ? '未登录' : '未知' }
function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
