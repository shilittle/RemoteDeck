import { Component, StrictMode } from 'react'
import type { ErrorInfo, ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import '@xterm/xterm/css/xterm.css'
import './styles.css'

class ErrorBoundary extends Component<{ children: ReactNode }, { error: string | null }> {
  public state: { error: string | null } = { error: null }
  public static getDerivedStateFromError(error: unknown): { error: string } {
    return { error: error instanceof Error ? error.message : '界面发生未知错误。' }
  }
  public componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('RemoteDeck renderer error', error.name, info.componentStack)
  }
  public render(): ReactNode {
    return this.state.error
      ? <main className="state-screen"><h1>界面遇到错误</h1><p>{this.state.error}</p><button className="primary" onClick={() => window.location.reload()}>重新加载界面</button></main>
      : this.props.children
  }
}

const root = document.getElementById('root')
if (!root) throw new Error('RemoteDeck root element is missing')
createRoot(root).render(<StrictMode><ErrorBoundary><App /></ErrorBoundary></StrictMode>)
