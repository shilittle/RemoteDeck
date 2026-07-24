import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './styles.css'

function App(): React.JSX.Element {
  return (
    <main className="shell">
      <div className="mark" aria-hidden="true">R</div>
      <h1>RemoteDeck</h1>
      <p>Windows SSH 工作台正在初始化</p>
    </main>
  )
}

const root = document.getElementById('root')
if (!root) throw new Error('Renderer root is missing')
createRoot(root).render(<StrictMode><App /></StrictMode>)

