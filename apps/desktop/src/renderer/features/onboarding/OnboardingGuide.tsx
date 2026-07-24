import { useState } from 'react'
import { ArrowRight, Import, Rocket, X } from 'lucide-react'
import { useHostStore } from '../../host-store'
import { useAppStore } from '../../store'

export function OnboardingGuide(): React.JSX.Element | null {
  const settings = useAppStore((state) => state.settings)
  const updateSettings = useAppStore((state) => state.updateSettings)
  const setActivity = useAppStore((state) => state.setActivity)
  const hosts = useHostStore((state) => state.items)
  const [welcomeOpen, setWelcomeOpen] = useState(true)
  if (!settings || settings.onboardingCompleted) return null
  const online = hosts.some((item) => item.state === 'online')
  const finish = (): void => { void updateSettings({ onboardingCompleted: true }) }
  if (welcomeOpen && hosts.length === 0) return <div className="onboarding-backdrop"><section className="onboarding-dialog" role="dialog" aria-modal="true" aria-labelledby="onboarding-title"><button className="onboarding-close" aria-label="跳过首次向导" onClick={finish}><X size={15} /></button><Rocket size={34} /><h1 id="onboarding-title">欢迎使用 RemoteDeck</h1><p>从添加 Linux 主机、确认指纹和登录，到打开 Codex，全程只需这个桌面应用。密码和私钥口令不会保存。</p><div className="onboarding-actions"><button className="primary" onClick={() => { setWelcomeOpen(false); setActivity('hosts') }}><ArrowRight size={14} />添加第一台主机</button><button onClick={() => { setWelcomeOpen(false); setActivity('settings') }}><Import size={14} />迁移 LabPulse 配置</button></div><small>稍后可在“设置”中重新导入旧配置；导入前会显示完整预览。</small></section></div>
  return <div className="onboarding-progress"><span><Rocket size={14} /><strong>首次设置</strong>{hosts.length === 0 ? '添加一台 Linux 主机' : online ? '主机已连接，打开 Codex 工作区' : '确认主机指纹并完成 SSH 登录'}</span><div>{hosts.length === 0 ? <button onClick={() => setActivity('hosts')}>添加主机</button> : online ? <button onClick={() => setActivity('commands')}>打开 Codex <ArrowRight size={12} /></button> : <button onClick={() => setActivity('hosts')}>继续连接</button>}<button onClick={finish}>完成向导</button></div></div>
}
