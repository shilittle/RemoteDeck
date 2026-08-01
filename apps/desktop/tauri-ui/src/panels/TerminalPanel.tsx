import { useCallback, useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { Clipboard, ClipboardPaste, Plus, RefreshCw, Search, SquarePen, X } from 'lucide-react'
import { api, errorMessage, events, sendTerminalInput } from '../api'
import { queuePendingTerminalOutput } from '../terminal-buffer'
import { mergeTerminalEvent, mergeTerminalListing } from '../terminal-events'
import { useAppStore } from '../store'
import type { TerminalEvent, TerminalSnapshot } from '../types'

interface TerminalHandle {
  terminal: Terminal
  fit: FitAddon
  search: SearchAddon
}

export function TerminalPanel({ hidden }: { hidden: boolean }): React.JSX.Element {
  const hosts = useAppStore((state) => state.hosts)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const settings = useAppStore((state) => state.settings)
  const reportError = useAppStore((state) => state.reportError)
  const selected = hosts.find((host) => host.id === selectedHostId) ?? null
  const [tabs, setTabs] = useState<TerminalSnapshot[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [searchText, setSearchText] = useState('')
  const [renamingId, setRenamingId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const handles = useRef(new Map<string, TerminalHandle>())
  const pendingOutput = useRef(new Map<string, string>())

  const addTab = useCallback((snapshot: TerminalSnapshot): void => {
    setTabs((current) => current.some((tab) => tab.sessionId === snapshot.sessionId)
      ? current.map((tab) => tab.sessionId === snapshot.sessionId ? { ...tab, ...snapshot } : tab)
      : [...current, snapshot])
    setActiveId(snapshot.sessionId)
  }, [])

  const createTab = useCallback(async (): Promise<void> => {
    if (!selectedHostId) return
    setBusy(true)
    setError('')
    try {
      const snapshot = await api.startTerminal(selectedHostId, 24, 80)
      addTab(snapshot)
    } catch (reason) {
      setError(errorMessage(reason))
    } finally {
      setBusy(false)
    }
  }, [addTab, selectedHostId])

  useEffect(() => {
    const lifecycle = { disposed: false }
    let cleanup: (() => void) | undefined
    let initialized = false
    const buffered: TerminalEvent[] = []
    const dispatch = (event: TerminalEvent): void => {
      applyTerminalEvent(event, handles.current, pendingOutput.current, setTabs, setError)
      const snapshot = event.snapshot
      if (snapshot) setActiveId((current) => current ?? snapshot.sessionId)
    }
    void (async () => {
      try {
        const next = await events.terminal((event) => {
          if (initialized) dispatch(event)
          else buffered.push(event)
        })
        if (lifecycle.disposed) {
          next()
          return
        }
        cleanup = next
      } catch (reason) {
        if (!lifecycle.disposed) {
          const message = errorMessage(reason)
          setError(message)
          reportError(message)
        }
      }

      let sessions: TerminalSnapshot[] = []
      try {
        sessions = await api.listTerminals()
        if (!lifecycle.disposed) setTabs((current) => mergeTerminalListing(current, sessions))
      } catch (reason) {
        if (!lifecycle.disposed) {
          const message = errorMessage(reason)
          setError(message)
          reportError(message)
        }
      }
      if (lifecycle.disposed) return
      initialized = true
      const bufferedActive = buffered.find((event) => event.snapshot)?.snapshot?.sessionId
      for (const event of buffered) dispatch(event)
      buffered.length = 0
      const listedActive = sessions.length > 0 ? sessions[0].sessionId : null
      setActiveId((current) => current ?? listedActive ?? bufferedActive ?? null)
    })()
    return () => { lifecycle.disposed = true; cleanup?.() }
  }, [reportError])

  useEffect(() => {
    const create = (): void => { void createTab() }
    const add = (event: Event): void => addTab((event as CustomEvent<TerminalSnapshot>).detail)
    window.addEventListener('remotedeck:new-terminal', create)
    window.addEventListener('remotedeck:terminal-created', add)
    return () => {
      window.removeEventListener('remotedeck:new-terminal', create)
      window.removeEventListener('remotedeck:terminal-created', add)
    }
  }, [addTab, createTab])

  useEffect(() => {
    if (!hidden && activeId) window.setTimeout(() => handles.current.get(activeId)?.fit.fit(), 0)
  }, [activeId, hidden])

  const active = tabs.find((tab) => tab.sessionId === activeId) ?? null

  const closeTab = async (sessionId: string): Promise<void> => {
    try { await api.closeTerminal(sessionId) } catch (reason) { setError(errorMessage(reason)) }
    setTabs((current) => {
      const next = current.filter((tab) => tab.sessionId !== sessionId)
      setActiveId((currentId) => currentId === sessionId ? next[0]?.sessionId ?? null : currentId)
      return next
    })
    const handle = handles.current.get(sessionId)
    handle?.terminal.dispose()
    handles.current.delete(sessionId)
    pendingOutput.current.delete(sessionId)
  }

  const reconnect = async (tab: TerminalSnapshot): Promise<void> => {
    const handle = handles.current.get(tab.sessionId)
    setBusy(true)
    setError('')
    try {
      const next = await api.reconnectTerminal(tab.sessionId, handle?.terminal.rows ?? 24, handle?.terminal.cols ?? 80)
      handle?.terminal.clear()
      handle?.terminal.writeln(`[RemoteDeck] 正在重新连接 ${next.alias}…`)
      addTab(next)
    } catch (reason) { setError(errorMessage(reason)) } finally { setBusy(false) }
  }

  const commitRename = (sessionId: string, title: string): void => {
    const next = title.trim()
    if (next) setTabs((current) => current.map((tab) => tab.sessionId === sessionId ? { ...tab, title: next } : tab))
    setRenamingId(null)
  }

  const copySelection = async (): Promise<void> => {
    if (!activeId) return
    const text = handles.current.get(activeId)?.terminal.getSelection() ?? ''
    if (text) await navigator.clipboard.writeText(text)
  }

  const pasteClipboard = async (): Promise<void> => {
    if (!activeId) return
    const text = await navigator.clipboard.readText()
    if (text) handles.current.get(activeId)?.terminal.paste(text)
  }

  return (
    <section className="terminal-workspace" hidden={hidden}>
      <div className="terminal-tabs" role="tablist" aria-label="终端标签">
        {tabs.map((tab) => (
          <div key={tab.sessionId} role="tab" tabIndex={0} aria-selected={tab.sessionId === activeId} className={tab.sessionId === activeId ? 'terminal-tab active' : 'terminal-tab'} onClick={() => setActiveId(tab.sessionId)} onDoubleClick={() => setRenamingId(tab.sessionId)} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') setActiveId(tab.sessionId) }}>
            <span className={`status-dot status-${tab.state}`} />
            {renamingId === tab.sessionId
              ? <input className="terminal-tab-rename" aria-label="终端标签名称" autoFocus defaultValue={tab.title || tab.alias} onClick={(event) => event.stopPropagation()} onBlur={(event) => commitRename(tab.sessionId, event.target.value)} onKeyDown={(event) => { event.stopPropagation(); if (event.key === 'Enter') commitRename(tab.sessionId, event.currentTarget.value); if (event.key === 'Escape') setRenamingId(null) }} />
              : <span>{tab.title || tab.alias}</span>}
            <small>{terminalStateLabel(tab.state)}</small>
            <button className="terminal-tab-close" aria-label={`关闭终端 ${tab.title || tab.alias}`} onClick={(event) => { event.stopPropagation(); void closeTab(tab.sessionId) }}><X size={13} /></button>
          </div>
        ))}
        <button className="terminal-icon-button" title="新建终端" aria-label="新建终端" disabled={!selectedHostId || busy} onClick={() => { void createTab() }}><Plus size={15} /></button>
      </div>

      <div className="terminal-toolbar">
        <span className="terminal-context">{active ? `${active.alias} · ${active.cwd}` : selected ? `${selected.alias} · 尚未打开终端` : '未选择主机'}</span>
        <label className="terminal-search"><Search size={14} /><input aria-label="终端搜索" placeholder="搜索滚屏" value={searchText} onChange={(event) => { const value = event.target.value; setSearchText(value); if (activeId) handles.current.get(activeId)?.search.findNext(value, { incremental: true }) }} onKeyDown={(event) => { if (event.key === 'Enter' && activeId) { if (event.shiftKey) handles.current.get(activeId)?.search.findPrevious(searchText); else handles.current.get(activeId)?.search.findNext(searchText) } }} /></label>
        <button className="terminal-icon-button" title="复制选择" aria-label="复制选择" disabled={!activeId} onClick={() => { void copySelection().catch((reason: unknown) => setError(errorMessage(reason))) }}><Clipboard size={15} /></button>
        <button className="terminal-icon-button" title="粘贴" aria-label="粘贴" disabled={!activeId} onClick={() => { void pasteClipboard().catch((reason: unknown) => setError(errorMessage(reason))) }}><ClipboardPaste size={15} /></button>
        {active && <button className="terminal-icon-button" title="重命名标签" aria-label="重命名标签" onClick={() => setRenamingId(active.sessionId)}><SquarePen size={15} /></button>}
        {active && <button className="terminal-reconnect" disabled={busy} onClick={() => { void reconnect(active) }}><RefreshCw size={14} />新建 shell 重连</button>}
      </div>

      {error && <div className="terminal-error" role="alert">{error}<button onClick={() => setError('')}>关闭</button></div>}
      {tabs.length === 0
        ? <div className="terminal-empty"><h1>尚未打开 SSH 终端</h1><p>{selected ? `将在 ${selected.alias} 的 ${selected.defaultWorkspace || '~'} 打开真实 OpenSSH + ConPTY 会话。认证提示直接显示在终端内。` : '请先选择并信任一台主机。'}</p><button className="primary" disabled={!selected || busy} onClick={() => { void createTab() }}><Plus size={15} />新建终端</button></div>
        : <div className="terminal-surfaces">{tabs.map((tab) => <TerminalSurface key={tab.sessionId} tab={tab} active={tab.sessionId === activeId} hidden={hidden} fontFamily={settings.terminalFontFamily} fontSize={settings.terminalFontSize} onReady={(handle) => { handles.current.set(tab.sessionId, handle); const pending = pendingOutput.current.get(tab.sessionId); if (pending) { pendingOutput.current.delete(tab.sessionId); handle.terminal.write(pending) } }} onError={setError} />)}</div>}
      {active && active.state !== 'running' && active.state !== 'starting' && <div className="terminal-offline">会话状态：{terminalStateLabel(active.state)}。可使用“新建 shell 重连”。</div>}
    </section>
  )
}

function TerminalSurface({ tab, active, hidden, fontFamily, fontSize, onReady, onError }: { tab: TerminalSnapshot; active: boolean; hidden: boolean; fontFamily: string; fontSize: number; onReady: (handle: TerminalHandle) => void; onError: (message: string) => void }): React.JSX.Element {
  const container = useRef<HTMLDivElement | null>(null)
  const handle = useRef<TerminalHandle | null>(null)
  const callbacks = useRef({ onReady, onError })
  callbacks.current = { onReady, onError }

  useEffect(() => {
    if (!container.current) return
    const terminal = new Terminal({
      cursorBlink: true,
      convertEol: false,
      fontFamily,
      fontSize,
      scrollback: 20_000,
      allowProposedApi: false,
      theme: { background: '#090d12', foreground: '#d7dde5', cursor: '#58a6ff', selectionBackground: '#264f78aa' }
    })
    const fit = new FitAddon()
    const search = new SearchAddon()
    terminal.loadAddon(fit)
    terminal.loadAddon(search)
    terminal.loadAddon(new Unicode11Addon())
    terminal.unicode.activeVersion = '11'
    terminal.open(container.current)
    fit.fit()
    const next = { terminal, fit, search }
    handle.current = next
    callbacks.current.onReady(next)
    const resize = terminal.onResize(({ cols, rows }) => { void api.resizeTerminal(tab.sessionId, rows, cols).catch(() => undefined) })
    const observer = new ResizeObserver(() => fit.fit())
    observer.observe(container.current)
    return () => { observer.disconnect(); resize.dispose(); terminal.dispose(); handle.current = null }
  }, [tab.sessionId])

  useEffect(() => {
    if (tab.state !== 'running') return
    let disposed = false
    let socket: WebSocket | undefined
    let input: { dispose: () => void } | undefined
    let reported = false
    const report = (message: string): void => {
      if (!disposed && !reported) {
        reported = true
        callbacks.current.onError(message)
      }
    }
    void api.openTerminalInput(tab.sessionId).then((endpoint) => {
      if (disposed) return
      const next = new WebSocket(endpoint.url)
      socket = next
      next.binaryType = 'arraybuffer'
      next.addEventListener('open', () => {
        if (disposed) {
          next.close()
          return
        }
        input = handle.current?.terminal.onData((value) => {
          try {
            sendTerminalInput(next, value)
          } catch (reason) {
            report(errorMessage(reason))
          }
        })
      }, { once: true })
      next.addEventListener('close', (event) => {
        input?.dispose()
        input = undefined
        if (!event.wasClean) report('安全终端输入通道意外关闭，请重新连接终端。')
      })
      next.addEventListener('error', () => report('无法建立安全终端输入通道，请重新连接终端。'), { once: true })
    }).catch((reason: unknown) => report(errorMessage(reason)))
    return () => {
      disposed = true
      input?.dispose()
      if (socket && socket.readyState < WebSocket.CLOSING) socket.close()
    }
  }, [tab.sessionId, tab.state])

  useEffect(() => {
    if (!handle.current) return
    handle.current.terminal.options.fontFamily = fontFamily
    handle.current.terminal.options.fontSize = fontSize
    handle.current.fit.fit()
  }, [fontFamily, fontSize])

  useEffect(() => { if (active && !hidden) window.setTimeout(() => { handle.current?.fit.fit(); handle.current?.terminal.focus() }, 0) }, [active, hidden])

  return <div className={active ? 'terminal-surface active' : 'terminal-surface'}><div className="xterm-host" ref={container} /></div>
}

function applyTerminalEvent(event: TerminalEvent, handles: Map<string, TerminalHandle>, pendingOutput: Map<string, string>, setTabs: React.Dispatch<React.SetStateAction<TerminalSnapshot[]>>, setError: (message: string) => void): void {
  const handle = handles.get(event.sessionId)
  if (event.kind === 'output' && event.data) {
    if (handle) handle.terminal.write(event.data)
    else queuePendingTerminalOutput(pendingOutput, event.sessionId, event.data)
  }
  if (event.kind === 'error' && event.message) {
    handle?.terminal.writeln(`\r\n[RemoteDeck] ${event.message}`)
    setError(event.message)
  }
  setTabs((current) => mergeTerminalEvent(current, event))
}

function terminalStateLabel(state: TerminalSnapshot['state']): string {
  return { starting: '连接中', running: '在线', offline: '离线', closed: '已关闭', failed: '失败' }[state]
}
