import { useState } from 'react'
import { api, errorMessage } from '../api'
import { useAppStore } from '../store'
import type { CommandResult } from '../types'

export function CommandPanel(): React.JSX.Element {
  const selectedHostId = useAppStore((state) => state.selectedHostId)
  const selected = useAppStore((state) => state.hosts.find((host) => host.id === state.selectedHostId) ?? null)
  const [command, setCommand] = useState('pwd && hostname && uname -a')
  const [directory, setDirectory] = useState('')
  const [result, setResult] = useState<CommandResult | null>(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const run = async (): Promise<void> => {
    if (!selectedHostId || !command.trim()) return
    setBusy(true); setResult(null); setMessage('')
    try { setResult(await api.runCommand(selectedHostId, command, directory)) }
    catch (error) { setMessage(errorMessage(error)) }
    finally { setBusy(false) }
  }
  return <section className="panel"><div className="panel-heading"><div><h1>一次性命令</h1><p>目标：{selected?.alias ?? '未选择'}。命令按原样交给远端登录 shell；执行前请自行审阅。</p></div><button className="primary" disabled={!selectedHostId || busy || !command.trim()} onClick={() => void run()}>{busy ? '执行中…' : '执行'}</button></div><div className="command-editor"><label><span>工作目录（可选）</span><input value={directory} onChange={(event) => setDirectory(event.target.value)} placeholder="~/project" /></label><label><span>命令</span><textarea value={command} onChange={(event) => setCommand(event.target.value)} spellCheck={false} /></label></div>{result && <div className="command-result"><div><strong>退出码 {result.exitCode ?? '未知'}</strong><small>{result.durationMs} ms</small></div>{result.stdout && <pre>{result.stdout}</pre>}{result.stderr && <pre className="stderr">{result.stderr}</pre>}</div>}{message && <p className="inline-message">{message}</p>}</section>
}
