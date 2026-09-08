import { useCallback, useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { Clipboard, ClipboardPaste, Plus, RefreshCw, Search, SquarePen, X } from 'lucide-react'
import { api, errorMessage, events, sendTerminalInput, terminalWebSocketUrl } from '../api'
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
  const selectHost = useAppStore((state) => state.selectHost)
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
    setTabs((current) => mergeTerminalListing(current, [snapshot]))
    selectHost(snapshot.hostId)
    setActiveId(snapshot.sessionId)
  }, [selectHost])

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
    let disposed = false
    let epoch = 0
    let buffered: TerminalEvent[] | null = null
    const dispatch = (event: TerminalEvent): void => {
      if (disposed) return
      buffered?.push(event)
      applyTerminalEvent(event, handles.current, pendingOutput.current, setTabs, setError)
    }
    const refresh = async (): Promise<void> => {
      const requestEpoch = ++epoch
      buffered = []
      try {
        const sessions = await api.listTerminals()
        if (disposed || requestEpoch !== epoch) return
        const changes = buffered
        buffered = null
        setTabs((current) => {
          // The list owns membership; revisions own ordering within a session.
          const ids = new Set(sessions.map((item) => item.sessionId))
          let next = mergeTerminalListing(current.filter((item) => ids.has(item.sessionId)), sessions)
          for (const event of changes) next = mergeTerminalEvent(next, event)
          return next
        })
      } catch (reason) {
        if (requestEpoch === epoch) buffered = null
        if (!disposed) setError(errorMessage(reason))
      }
    }
    const cleanup = events.terminal(dispatch)
    const resync = events.resync(() => { void refresh() })
    void refresh()
    return () => { disposed = true; cleanup(); resync() }
  }, [reportError])

  const visibleTabs = tabs.filter((tab) => tab.hostId === selectedHostId)
  useEffect(() => {
    setActiveId((current) => tabs.some((tab) => tab.sessionId === current && tab.hostId === selectedHostId)
      ? current : tabs.find((tab) => tab.hostId === selectedHostId)?.sessionId ?? null)
  }, [selectedHostId, tabs])

  useEffect(() => {
    const create = (): void => { void createTab() }
    const add = (event: Event): void => addTab((event as CustomEvent<TerminalSnapshot>).detail)
    const focus = (event: Event): void => {
      const id = (event as CustomEvent<{ sessionId: string }>).detail.sessionId
      setActiveId(id)
      void api.listTerminals().then((sessions) => {
        const session = sessions.find((item) => item.sessionId === id)
        if (session) addTab(session)
      }).catch((reason: unknown) => setError(errorMessage(reason)))
    }
    window.addEventListener('remotedeck:new-terminal', create)
    window.addEventListener('remotedeck:terminal-created', add)
    window.addEventListener('remotedeck:focus-terminal', focus)
    return () => {
      window.removeEventListener('remotedeck:new-terminal', create)
      window.removeEventListener('remotedeck:terminal-created', add)
      window.removeEventListener('remotedeck:focus-terminal', focus)
    }
  }, [addTab, createTab])

  useEffect(() => {
    if (!hidden && activeId) window.setTimeout(() => handles.current.get(activeId)?.fit.fit(), 0)
  }, [activeId, hidden])

  const active = visibleTabs.find((tab) => tab.sessionId === activeId) ?? null

  const closeTab = async (sessionId: string): Promise<void> => {
    try { await api.closeTerminal(sessionId) } catch (reason) { setError(errorMessage(reason)); return }
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
        {visibleTabs.map((tab) => (
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
      {visibleTabs.length === 0
        ? <div className="terminal-empty"><h1>尚未打开 SSH 终端</h1><p>{selected ? `将在 ${selected.alias} 的 ${selected.defaultWorkspace || '~'} 打开真实 OpenSSH + ConPTY 会话。认证提示直接显示在终端内。` : '请先选择并信任一台主机。'}</p><button className="primary" disabled={!selected || busy} onClick={() => { void createTab() }}><Plus size={15} />新建终端</button></div>
        : <div className="terminal-surfaces">{tabs.map((tab) => <TerminalSurface key={tab.sessionId} tab={tab} active={tab.sessionId === activeId} hidden={hidden} fontFamily={settings.terminalFontFamily} fontSize={settings.terminalFontSize} onReady={(handle) => { handles.current.set(tab.sessionId, handle); const pending = pendingOutput.current.get(tab.sessionId); if (pending) { pendingOutput.current.delete(tab.sessionId); handle.terminal.write(pending) } }} onError={setError} />)}</div>}
      {active && active.state !== 'running' && active.state !== 'starting' && <div className="terminal-offline">会话状态：{terminalStateLabel(active.state)}。可使用“新建 shell 重连”。</div>}
    </section>
  )
}

function TerminalSurface({ tab, active, hidden, fontFamily, fontSize, onReady, onError }: { tab: TerminalSnapshot; active: boolean; hidden: boolean; fontFamily: string; fontSize: number; onReady: (handle: TerminalHandle) => void; onError: (message: string) => void }): React.JSX.Element {
  const container = useRef<HTMLDivElement | null>(null)
  const handle = useRef<TerminalHandle | null>(null)
  const inputSocket = useRef<WebSocket | null>(null)
  const cursor = useRef({ generation: -1, sequence: 0 })
  const currentLease = useRef<number | null>(null)
  const resumeLease = useRef<number | null>(null)
  const [connectionVersion, setConnectionVersion] = useState(0)
  const [connection, setConnection] = useState('尚未附着终端')
  const [hasControl, setHasControl] = useState(false)
  const callbacks = useRef({ onReady, onError })
  callbacks.current = { onReady, onError }

  useEffect(() => {
    if (!container.current) return
    cursor.current = { generation: -1, sequence: 0 }
    const terminal = new Terminal({
      cursorBlink: true,
      convertEol: false,
      fontFamily,
      fontSize,
      scrollback: 20_000,
      // Unicode11Addon uses xterm's unicode provider API for CJK cell widths.
      allowProposedApi: true,
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
    const resize = terminal.onResize(({ cols, rows }) => {
      const socket = inputSocket.current
      if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: 'resize', rows, cols }))
    })
    const observer = new ResizeObserver(() => fit.fit())
    observer.observe(container.current)
    return () => { observer.disconnect(); resize.dispose(); terminal.dispose(); handle.current = null }
  }, [tab.sessionId])

  useEffect(() => {
    if (tab.state !== 'running' || !active) return
    let disposed = false
    let socket: WebSocket | undefined
    let input: { dispose: () => void } | undefined
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined
    let retry = false
    const expectedLease = resumeLease.current
    resumeLease.current = null
    setHasControl(false)
    setConnection('正在附着已有终端…')
    const reattach = (): void => {
      if (disposed || reconnectTimer) return
      if (currentLease.current === null) {
        setConnection('附着过程已中断，请手动重新附着。')
        return
      }
      resumeLease.current = currentLease.current
      setConnection('连接中断，正在重新附着已有终端…')
      reconnectTimer = setTimeout(() => { if (!disposed) setConnectionVersion((value) => value + 1) }, 1500)
    }
    void api.openTerminalInput(tab.sessionId).then((endpoint) => {
      if (disposed) return
      if (expectedLease !== null && endpoint.generation !== cursor.current.generation) {
        setConnection('终端已重建，请手动重新附着。')
        return
      }
      if (cursor.current.generation !== endpoint.generation) {
        if (cursor.current.generation !== -1) handle.current?.terminal.reset()
        cursor.current = { generation: endpoint.generation, sequence: 0 }
      }
      const url = new URL(terminalWebSocketUrl(endpoint))
      if (expectedLease !== null) url.searchParams.set('resumeLease', String(expectedLease))
      if (cursor.current.sequence > 0) url.searchParams.set('since', String(cursor.current.sequence))
      const next = new WebSocket(url)
      socket = next
      inputSocket.current = next
      next.binaryType = 'arraybuffer'
      let attached = false
      next.addEventListener('message', (message) => {
        if (typeof message.data !== 'string' || disposed) return
        try {
          const envelope = JSON.parse(message.data) as { type?: unknown; message?: unknown; sessionId?: unknown; sequence?: unknown; generation?: unknown; lease?: unknown; event?: TerminalEvent }
          if (envelope.type === 'resync') { retry = true; next.close(); return }
          if (envelope.type === 'error') {
            setConnection(typeof envelope.message === 'string' ? envelope.message : '终端附着失败，请重新附着。')
            next.close(); return
          }
          if (envelope.type === 'attached') {
            if (envelope.generation !== cursor.current.generation || !Number.isSafeInteger(envelope.lease) || envelope.sessionId !== tab.sessionId || attached) throw new Error('invalid terminal attachment')
            currentLease.current = envelope.lease as number
            attached = true
            setConnection('正在回放终端输出…')
            return
          }
          if (envelope.type === 'replay_complete') {
            if (!attached) throw new Error('replay completed before attachment')
            const terminal = handle.current?.terminal
            // Historical terminal queries must not send fresh DSR/DA replies into
            // the existing shell. Drain xterm's replay queue before enabling input.
            terminal?.write('', () => {
              if (disposed || next.readyState !== WebSocket.OPEN) return
              input = terminal.onData((value) => {
                try { sendTerminalInput(next, value, endpoint) } catch (reason) { callbacks.current.onError(errorMessage(reason)) }
              })
              next.send(JSON.stringify({ type: 'resize', rows: terminal.rows, cols: terminal.cols }))
              setConnection('已获得输入控制权')
              setHasControl(true)
            })
            return
          }
          if (!attached || !Number.isSafeInteger(envelope.sequence) || envelope.generation !== cursor.current.generation || !envelope.event) throw new Error('invalid terminal message')
          const sequence = envelope.sequence as number
          const event = envelope.event
          if (event.sessionId !== tab.sessionId) throw new Error('terminal session mismatch')
          if (sequence <= cursor.current.sequence) return
          if (sequence !== cursor.current.sequence + 1 && event.kind !== 'replayTruncated') {
            retry = true; next.close(); return
          }
          cursor.current.sequence = sequence
          if (event.kind === 'output' && event.data) handle.current?.terminal.write(event.data)
          if (event.kind === 'replayTruncated') handle.current?.terminal.writeln(`\r\n[RemoteDeck] ${event.message ?? '已回放最近 1 MiB 输出；更早内容已截断。'}`)
          if (event.kind === 'error' && event.message) callbacks.current.onError(event.message)
        } catch {
          setConnection('终端数据校验失败，请重新附着。')
          next.close()
        }
      })
      next.addEventListener('close', (event) => {
        input?.dispose(); input = undefined
        if (disposed) return
        setHasControl(false)
        if (event.code === 4001) setConnection('输入已由其他页面接管。可在此接管输入。')
        else if (event.code === 4003) setConnection('浏览器会话已失效，请从 RemoteDeck 快捷方式重新打开。')
        else if (event.code === 4000) setConnection('本机服务已停止。')
        else if (retry || event.code === 1006) reattach()
        else setConnection('终端通道已断开，可重新附着已有终端。')
      })
      next.addEventListener('error', () => { if (!disposed) setConnection('终端通道暂时不可用。') })
    }).catch((reason: unknown) => {
      if (disposed) return
      setConnection(errorMessage(reason))
      // A network failure can recover; an explicit permission/state error needs user action.
      if (reason instanceof TypeError) reattach()
    })
    return () => {
      disposed = true
      clearTimeout(reconnectTimer)
      input?.dispose()
      if (inputSocket.current === socket) inputSocket.current = null
      if (socket && socket.readyState < WebSocket.CLOSING) socket.close()
    }
  }, [active, tab.sessionId, tab.state, tab.generation, connectionVersion])

  useEffect(() => {
    if (!handle.current) return
    handle.current.terminal.options.fontFamily = fontFamily
    handle.current.terminal.options.fontSize = fontSize
    handle.current.fit.fit()
  }, [fontFamily, fontSize])

  useEffect(() => { if (active && !hidden) window.setTimeout(() => { handle.current?.fit.fit(); handle.current?.terminal.focus() }, 0) }, [active, hidden])

  return <div className={active ? 'terminal-surface active' : 'terminal-surface'}>
    <div className="terminal-attachment"><span role="status">{connection}</span>{tab.state === 'running' && <button disabled={hasControl} onClick={() => { resumeLease.current = null; setConnectionVersion((value) => value + 1) }}>{connection.includes('接管') ? '接管输入' : '重新附着'}</button>}</div>
    <div className="xterm-host" ref={container} />
  </div>
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
  return { starting: '启动中', running: '运行中', offline: '已退出', closed: '已关闭', failed: '失败' }[state]
}
