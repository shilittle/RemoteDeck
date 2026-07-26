import { useEffect, useState } from 'react'
import { useAppStore } from '../store'

export function SettingsPanel(): React.JSX.Element {
  const settings = useAppStore((state) => state.settings)
  const updateSettings = useAppStore((state) => state.updateSettings)
  const busy = useAppStore((state) => state.busy)
  const [fontFamily, setFontFamily] = useState(settings?.terminalFontFamily ?? '')
  const [fontSize, setFontSize] = useState(settings?.terminalFontSize ?? 14)
  useEffect(() => { if (settings) { setFontFamily(settings.terminalFontFamily); setFontSize(settings.terminalFontSize) } }, [settings])
  return <section className="panel"><div className="panel-heading"><div><h1>设置</h1><p>当前切片只保留会真实影响运行时的设置。</p></div><button className="primary" disabled={busy || !settings} onClick={() => void updateSettings({ terminalFontFamily: fontFamily, terminalFontSize: fontSize })}>保存</button></div><div className="form-grid"><label className="wide"><span>终端字体</span><input value={fontFamily} onChange={(event) => setFontFamily(event.target.value)} /></label><label><span>终端字号</span><input type="number" min={9} max={32} value={fontSize} onChange={(event) => setFontSize(Number(event.target.value))} /></label></div><div className="architecture-note"><strong>发行边界</strong><p>只构建 Tauri NSIS。没有 Electron、没有 portable 自解压、没有内嵌 WebView2 离线运行时。</p></div></section>
}
