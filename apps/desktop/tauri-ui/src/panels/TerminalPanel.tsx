import { useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SearchAddon } from '@xterm/addon-search'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebLinksAddon } from '@xterm/addon-web-links'
import { api, listenTerminal } from '../api'
import { useAppStore } from '../store'
import type { TerminalSnapshot } from '../types'

export function TerminalPanel({ hidden }: { hidden: boolean }): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const sessionRef = useRef<TerminalSnapshot | null>(null)
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const hosts = useAppStore((state) => state.hosts)
  const settings = useAppStore((state) => state.settings)
  const reportError = useAppStore((state) => state.reportError)
  const [session, setSession] = useState<TerminalSnapshot | null>(null)
  const [busy, setBusy] = useState(false)
  const selected = hosts.find((host) => host.id === selectedHostId) ?? null

  useEffect(() => {
    if (!containerRef.current) return
    const terminal = new Terminal({
      cursorBlink: true,
      convertEol: false,
      fontFamily: settings?.terminalFontFamily,
      fontSize: settings?.terminalFontSize,
      scrollback: 10_000,
      theme: { background: '#0b0f14', foreground: '#d7dde5', cursor: '#f7812f' }
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.loadAddon(new SearchAddon())
    terminal.loadAddon(new Unicode11Addon())
    terminal.loadAddon(new WebLinksAddon())
    terminal.unicode.activeVersion = '11'
    terminal.open(containerRef.current)
    fit.fit()
    terminalRef.current = terminal
    fitRef.current = fit
    const dataSubscription = terminal.onData((data) => {
      const current = sessionRef.current
      if (current) void api.writeTerminal(current.sessionId, data).catch(reportError)
    })
    const observer = new ResizeObserver(() => {
      fit.fit()
      const current = sessionRef.current
      if (current) void api.resizeTerminal(current.sessionId, terminal.rows, terminal.cols).catch(() => undefined)
    })
    observer.observe(containerRef.current)
    let disposed = false
    let unlisten: (() => void) | undefined
    void listenTerminal((event) => {
      const current = sessionRef.current
      if (!current || event.sessionId !== current.sessionId) return
      if (event.kind === 'output' && event.data) terminal.write(event.data)
      if (event.kind === 'error' && event.message) terminal.writeln(`\r\n[RemoteDeck] ${event.message}`)
      if (event.kind === 'exit' || event.kind === 'error') { sessionRef.current = null; setSession(null) }
    }).then((cleanup) => { if (disposed) cleanup(); else unlisten = cleanup }).catch(reportError)
    return () => {
      disposed = true; unlisten?.(); observer.disconnect(); dataSubscription.dispose()
      const current = sessionRef.current
      if (current) void api.closeTerminal(current.sessionId).catch(() => undefined)
      terminal.dispose(); terminalRef.current = null; fitRef.current = null
    }
  }, [reportError])

  useEffect(() => {
    if (!terminalRef.current || !settings) return
    terminalRef.current.options.fontFamily = settings.terminalFontFamily
    terminalRef.current.options.fontSize = settings.terminalFontSize
    fitRef.current?.fit()
  }, [settings])

  useEffect(() => { if (!hidden) setTimeout(() => fitRef.current?.fit(), 0) }, [hidden])

  const connect = async (): Promise<void> => {
    if (!selectedHostId || !terminalRef.current) return
    setBusy(true)
    try {
      terminalRef.current.clear()
      terminalRef.current.writeln(`[RemoteDeck] connecting to ${selected?.alias ?? selectedHostId}…`)
      const snapshot = await api.startTerminal(selectedHostId, terminalRef.current.rows, terminalRef.current.cols)
      sessionRef.current = snapshot; setSession(snapshot); terminalRef.current.focus()
    } catch (error) { reportError(error) }
    finally { setBusy(false) }
  }
  const disconnect = async (): Promise<void> => {
    const current = sessionRef.current
    if (!current) return
    try { await api.closeTerminal(current.sessionId) }
    catch (error) { reportError(error) }
    finally { sessionRef.current = null; setSession(null) }
  }

  return <section className={hidden ? 'terminal-panel hidden' : 'terminal-panel'}><div className="terminal-toolbar"><div><strong>{session ? `${session.alias} · 已连接` : selected ? `${selected.alias} · 未连接` : '未选择主机'}</strong><small>OpenSSH + Windows ConPTY</small></div><div className="button-row">{session ? <button className="danger" onClick={() => void disconnect()}>断开</button> : <button className="primary" disabled={!selectedHostId || busy} onClick={() => void connect()}>{busy ? '连接中…' : '打开终端'}</button>}</div></div><div className="terminal-surface" ref={containerRef} /></section>
}
