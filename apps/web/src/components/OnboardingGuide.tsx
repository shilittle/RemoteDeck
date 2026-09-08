import { ArrowRight, Import, Rocket, X } from 'lucide-react'
import { useAppStore } from '../store'

export function OnboardingGuide(): React.JSX.Element | null {
  const settings = useAppStore((state) => state.settings)
  const hosts = useAppStore((state) => state.hosts)
  const setActivity = useAppStore((state) => state.setActivity)
  const updateSettings = useAppStore((state) => state.updateSettings)
  if (settings.onboardingCompleted) return null
  const finish = (): void => { void updateSettings({ onboardingCompleted: true }) }
  if (hosts.length === 0) {
    return <div className="onboarding-backdrop"><section className="onboarding-dialog" role="dialog" aria-modal="true" aria-labelledby="onboarding-title"><button className="onboarding-close" aria-label="跳过首次向导" onClick={finish}><X size={15} /></button><Rocket size={36} /><h1 id="onboarding-title">欢迎使用 RemoteDeck</h1><p>RemoteDeck 会沿用本机 SSH known_hosts 中已有的匹配信任；已有记录可以直接连接，只有新端点或指纹变化时才需要核对。</p><div className="onboarding-steps"><span><strong>1</strong>添加或导入主机</span><span><strong>2</strong>没有本地信任时扫描并核对</span><span><strong>3</strong>在终端内完成认证</span></div><div className="onboarding-actions"><button className="primary" onClick={() => { setActivity('hosts'); finish() }}><ArrowRight size={14} />添加第一台主机</button><button onClick={() => { setActivity('settings'); finish() }}><Import size={14} />迁移旧版配置</button></div><small>密码、交互回答和私钥口令不会写入配置、诊断或浏览器请求体。</small></section></div>
  }
  return <div className="onboarding-progress"><span><Rocket size={14} /><strong>首次设置</strong>已有本机 SSH 信任的主机可直接连接；新端点请先扫描核对。</span><div><button onClick={() => setActivity('hosts')}>继续连接</button><button onClick={finish}>完成向导</button></div></div>
}
