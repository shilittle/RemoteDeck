import { useCallback, useEffect, useRef, useState } from 'react'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { Terminal } from '@xterm/xterm'
import { Clipboard, ClipboardPaste, Plus, RefreshCw, Search, SquarePen, X } from 'lucide-react'
import type { TerminalSession } from '../../../protocol/terminal'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'
import '@xterm/xterm/css/xterm.css'

interface TerminalTab extends TerminalSession { name: string }
interface TerminalHandle { terminal: Terminal; fit: FitAddon; search: SearchAddon }

export function TerminalWorkspace({ hidden }: { hidden: boolean }): React.JSX.Element {
  const hosts = useHostStore((state) => state.items)
  const selectedId = useHostStore((state) => state.selectedId)
  const selected = hosts.find((item) => item.host.id === selectedId)
  const settings = useAppStore((state) => state.settings)
  const [tabs, setTabs] = useState<TerminalTab[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [searchText, setSearchText] = useState('')
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [error, setError] = useState('')
  const handles = useRef(new Map<string, TerminalHandle>())
  const pendingOutput = useRef(new Map<string, string>())
  const generations = useRef(new Map<string, number>())
  const creating = useRef(false)
  const tabsRef = useRef(tabs)
  tabsRef.current = tabs

  const createTab = useCallback(async (): Promise<void> => {
    const current = useHostStore.getState()
    const host = current.items.find((item) => item.host.id === current.selectedId)
    if (!host || host.state !== 'online') { setError('请先在“主机”中完成指纹确认并连接 SSH。'); return }
    if (creating.current) return
    creating.current = true
    setError('')
    try {
      const session = await window.remoteDeck.terminals.create({ hostId: host.host.id, cwd: host.workspace?.remotePath ?? '~', cols: 80, rows: 24 })
      generations.current.set(session.id, session.generation)
      setTabs((currentTabs) => currentTabs.some((tab) => tab.id === session.id) ? currentTabs : [...currentTabs, { ...session, name: `${session.hostAlias} shell ${String(currentTabs.filter((tab) => tab.hostId === session.hostId).length + 1)}` }])
      setActiveId(session.id)
      if (session.error) setError(session.error)
    } finally {
      creating.current = false
    }
  }, [])

  useEffect(() => {
    let disposed = false
    void window.remoteDeck.terminals.list().then((sessions) => {
      if (disposed) return
      for (const session of sessions) generations.current.set(session.id, session.generation)
      setTabs(sessions.map((session, index) => ({ ...session, name: `${session.hostAlias} shell ${String(index + 1)}` })))
      setActiveId((current) => current ?? sessions[0]?.id ?? null)
    }).catch((reason: unknown) => setError(messageOf(reason)))
    const unsubscribe = window.remoteDeck.terminals.onEvent((event) => {
      if (event.type === 'data') {
        if (generations.current.get(event.sessionId) !== event.generation) return
        const handle = handles.current.get(event.sessionId)
        if (handle) handle.terminal.write(event.data)
        else pendingOutput.current.set(event.sessionId, `${pendingOutput.current.get(event.sessionId) ?? ''}${event.data}`.slice(-4 * 1024 * 1024))
        return
      }
      generations.current.set(event.session.id, event.session.generation)
      setTabs((current) => {
        const existing = current.find((tab) => tab.id === event.session.id)
        if (!existing) { setActiveId(event.session.id); return [...current, { ...event.session, name: `${event.session.hostAlias} shell ${String(current.length + 1)}` }] }
        return current.map((tab) => tab.id === event.session.id ? { ...event.session, name: tab.name } : tab)
      })
    })
    return () => { disposed = true; unsubscribe() }
  }, [])

  useEffect(() => {
    if (!hidden && tabs.length === 0 && selected?.state === 'online') void createTab()
  }, [createTab, hidden, selected?.state, tabs.length])

  useEffect(() => {
    if (hidden || !activeId) return
    const frame = requestAnimationFrame(() => handles.current.get(activeId)?.fit.fit())
    return () => cancelAnimationFrame(frame)
  }, [activeId, hidden])

  function registerHandle(sessionId: string, handle: TerminalHandle | null): void {
    if (!handle) { handles.current.delete(sessionId); return }
    handles.current.set(sessionId, handle)
    const pending = pendingOutput.current.get(sessionId)
    if (pending) { handle.terminal.write(pending); pendingOutput.current.delete(sessionId) }
  }

  async function closeTab(sessionId: string): Promise<void> {
    setError('')
    try { await window.remoteDeck.terminals.close(sessionId) } catch (reason) { setError(messageOf(reason)) }
    const index = tabsRef.current.findIndex((tab) => tab.id === sessionId)
    const remaining = tabsRef.current.filter((tab) => tab.id !== sessionId)
    setTabs(remaining)
    if (activeId === sessionId) setActiveId(remaining[Math.max(0, index - 1)]?.id ?? remaining[0]?.id ?? null)
    generations.current.delete(sessionId)
    pendingOutput.current.delete(sessionId)
  }

  async function reconnect(tab: TerminalTab): Promise<void> {
    const host = hosts.find((item) => item.host.id === tab.hostId)
    if (host?.state !== 'online') { setError(`主机 ${tab.hostAlias} 已离线。请回到“主机”重新认证；普通 shell 不能恢复，只能建立新 shell。`); return }
    setError('')
    handles.current.get(tab.id)?.terminal.write('\r\n\x1b[33m—— RemoteDeck 正在建立新的 SSH shell（原滚屏仅供参考）——\x1b[0m\r\n')
    try {
      const session = await window.remoteDeck.terminals.reconnect(tab.id)
      generations.current.set(session.id, session.generation)
    } catch (reason) { setError(messageOf(reason)) }
  }

  function commitRename(tabId: string, value: string): void {
    const name = value.trim().slice(0, 80)
    if (name) setTabs((current) => current.map((item) => item.id === tabId ? { ...item, name } : item))
    setRenamingId(null)
  }

  async function copySelection(): Promise<void> {
    const text = activeId ? handles.current.get(activeId)?.terminal.getSelection() ?? '' : ''
    if (text) await window.remoteDeck.app.clipboardWrite(text)
  }

  async function pasteClipboard(): Promise<void> {
    if (!activeId) return
    const { text } = await window.remoteDeck.app.clipboardRead()
    if (text) handles.current.get(activeId)?.terminal.paste(text)
  }

  const active = tabs.find((tab) => tab.id === activeId)
  return (
    <section className="terminal-workspace" hidden={hidden} aria-label="SSH 终端工作区">
      <div className="terminal-tabs" role="tablist" aria-label="终端标签">
        {tabs.map((tab) => <div key={tab.id} role="tab" tabIndex={0} aria-selected={tab.id === activeId} className={tab.id === activeId ? 'terminal-tab active' : 'terminal-tab'} onClick={() => setActiveId(tab.id)} onDoubleClick={() => setRenamingId(tab.id)} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') setActiveId(tab.id) }}><span className={`status-dot status-${tab.state}`} />{renamingId === tab.id ? <input className="terminal-tab-rename" aria-label="终端标签名称" autoFocus defaultValue={tab.name} onClick={(event) => event.stopPropagation()} onBlur={(event) => commitRename(tab.id, event.target.value)} onKeyDown={(event) => { event.stopPropagation(); if (event.key === 'Enter') commitRename(tab.id, event.currentTarget.value); if (event.key === 'Escape') setRenamingId(null) }} /> : <span>{tab.name}</span>}<small>{tab.state}</small><button className="terminal-tab-close" aria-label={`关闭终端 ${tab.name}`} onClick={(event) => { event.stopPropagation(); void closeTab(tab.id) }}><X size={13} /></button></div>)}
        <button className="terminal-icon-button" title="新建终端" aria-label="新建终端" onClick={() => { void createTab() }}><Plus size={15} /></button>
      </div>
      <div className="terminal-toolbar">
        <span className="terminal-context">{active ? `${active.hostAlias} · ${active.cwd}` : selected ? `${selected.host.alias} · 尚未打开终端` : '未选择主机'}</span>
        <label className="terminal-search"><Search size={14} /><input aria-label="终端搜索" placeholder="搜索滚屏" value={searchText} onChange={(event) => { const value = event.target.value; setSearchText(value); if (activeId) handles.current.get(activeId)?.search.findNext(value, { incremental: true }) }} onKeyDown={(event) => { if (event.key === 'Enter' && activeId) { if (event.shiftKey) handles.current.get(activeId)?.search.findPrevious(searchText); else handles.current.get(activeId)?.search.findNext(searchText) } }} /></label>
        <button className="terminal-icon-button" title="复制选择" aria-label="复制选择" onClick={() => { void copySelection() }}><Clipboard size={15} /></button>
        <button className="terminal-icon-button" title="粘贴" aria-label="粘贴" onClick={() => { void pasteClipboard() }}><ClipboardPaste size={15} /></button>
        {active && <button className="terminal-icon-button" title="重命名标签" aria-label="重命名标签" onClick={() => setRenamingId(active.id)}><SquarePen size={15} /></button>}
        {active && <button className="terminal-reconnect" onClick={() => { void reconnect(active) }}><RefreshCw size={14} />新建 shell 重连</button>}
      </div>
      {error && <div className="terminal-error" role="alert">{error}</div>}
      {tabs.length === 0 ? <div className="terminal-empty"><h1>尚未打开 SSH 终端</h1><p>{selected?.state === 'online' ? `将在 ${selected.host.alias} 的 ${selected.workspace?.remotePath ?? '~'} 新建真实 PTY shell。` : '先选择主机、确认指纹并完成 SSH 认证。'}</p><button className="primary" disabled={selected?.state !== 'online'} onClick={() => { void createTab() }}><Plus size={15} />新建终端</button></div> : <div className="terminal-surfaces">{tabs.map((tab) => <TerminalSurface key={tab.id} tab={tab} active={tab.id === activeId} hidden={hidden} fontFamily={settings?.terminalFontFamily ?? 'Cascadia Mono'} fontSize={settings?.terminalFontSize ?? 14} onReady={(handle) => registerHandle(tab.id, handle)} onError={setError} onCopy={() => { void copySelection() }} onPaste={() => { void pasteClipboard() }} />)}</div>}
      {active && active.state !== 'online' && <div className="terminal-offline">{active.error ?? '该 shell 已断线。滚屏已保留，但普通 shell 无法恢复；重新认证后只能建立新的 shell。'}</div>}
    </section>
  )
}

function TerminalSurface({ tab, active, hidden, fontFamily, fontSize, onReady, onError, onCopy, onPaste }: { tab: TerminalTab; active: boolean; hidden: boolean; fontFamily: string; fontSize: number; onReady: (handle: TerminalHandle | null) => void; onError: (message: string) => void; onCopy: () => void; onPaste: () => void }): React.JSX.Element {
  const container = useRef<HTMLDivElement>(null)
  const handle = useRef<TerminalHandle | null>(null)
  const visibility = useRef({ active, hidden })
  const callbacks = useRef({ onReady, onError, onCopy, onPaste })
  const initialFont = useRef({ fontFamily, fontSize })
  visibility.current = { active, hidden }
  callbacks.current = { onReady, onError, onCopy, onPaste }
  useEffect(() => {
    if (!container.current) return
    const terminal = new Terminal({ allowProposedApi: true, cursorBlink: true, cursorStyle: 'bar', fontFamily: initialFont.current.fontFamily, fontSize: initialFont.current.fontSize, scrollback: 10_000, convertEol: false, theme: { background: '#090d12', foreground: '#d8dee9', cursor: '#58a6ff', selectionBackground: '#264f78' } })
    const fit = new FitAddon()
    const search = new SearchAddon()
    terminal.loadAddon(fit)
    terminal.loadAddon(search)
    terminal.loadAddon(new WebLinksAddon((_event, uri) => { void window.remoteDeck.app.openExternal(uri).catch((reason: unknown) => callbacks.current.onError(messageOf(reason))) }))
    terminal.loadAddon(new Unicode11Addon())
    terminal.unicode.activeVersion = '11'
    terminal.open(container.current)
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown' || !event.ctrlKey || !event.shiftKey) return true
      if (event.code === 'KeyC') { callbacks.current.onCopy(); return false }
      if (event.code === 'KeyV') { callbacks.current.onPaste(); return false }
      return true
    })
    const data = terminal.onData((value) => { void window.remoteDeck.terminals.write(tab.id, value).catch((reason: unknown) => callbacks.current.onError(messageOf(reason))) })
    const resize = terminal.onResize(({ cols, rows }) => { void window.remoteDeck.terminals.resize({ sessionId: tab.id, cols, rows }).catch((reason: unknown) => callbacks.current.onError(messageOf(reason))) })
    const resizeObserver = new ResizeObserver(() => { if (!visibility.current.hidden && visibility.current.active) fit.fit() })
    resizeObserver.observe(container.current)
    handle.current = { terminal, fit, search }
    callbacks.current.onReady(handle.current)
    const frame = requestAnimationFrame(() => fit.fit())
    return () => { cancelAnimationFrame(frame); resizeObserver.disconnect(); data.dispose(); resize.dispose(); terminal.dispose(); handle.current = null; callbacks.current.onReady(null) }
  }, [tab.id])
  useEffect(() => {
    if (!handle.current) return
    handle.current.terminal.options.fontFamily = fontFamily
    handle.current.terminal.options.fontSize = fontSize
    handle.current.fit.fit()
  }, [fontFamily, fontSize])
  useEffect(() => {
    if (!active || hidden) return
    const frame = requestAnimationFrame(() => { handle.current?.fit.fit(); handle.current?.terminal.focus() })
    return () => cancelAnimationFrame(frame)
  }, [active, hidden])
  return <div className={active ? 'terminal-surface active' : 'terminal-surface'} aria-hidden={!active}><div ref={container} className="xterm-host" /></div>
}

function messageOf(value: unknown): string { return value instanceof Error ? value.message : String(value) }
