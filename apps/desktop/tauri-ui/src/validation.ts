import type { HostDraft, TunnelDraft } from './types'

const aliasPattern = /^[A-Za-z0-9_.-]{1,64}$/

export function validateHostDraft(draft: HostDraft): string | null {
  if (!aliasPattern.test(draft.alias.trim())) return '别名只能包含字母、数字、点、下划线和短横线，长度 1–64。'
  if (!safeSshAtom(draft.hostname, 512)) return '主机名 / IP 不合法。'
  if (!safeSshAtom(draft.username, 128)) return '用户名不合法。'
  if (!Number.isInteger(draft.port) || draft.port < 1 || draft.port > 65_535) return '端口必须是 1–65535 的整数。'
  if (draft.proxyJump && !safeSshAtom(draft.proxyJump, 1024)) return 'ProxyJump 值不合法。'
  if ((draft.identityFile?.length ?? 0) > 32_767) return '私钥路径过长。'
  if ((draft.defaultWorkspace?.length ?? 0) > 4096) return '默认工作目录过长。'
  return null
}

export function validateTunnelDraft(draft: TunnelDraft): string | null {
  if (!draft.hostId) return '先选择主机。'
  if (!draft.name.trim() || draft.name.length > 128) return '隧道名称不能为空且不得超过 128 个字符。'
  if (!safeForwardAtom(draft.bindAddress)) return '监听地址不合法。'
  if (!safeForwardAtom(draft.targetHost)) return '目标主机不合法。'
  if (!validPort(draft.sourcePort) || !validPort(draft.targetPort)) return '源端口和目标端口必须是 1–65535 的整数。'
  return null
}

function safeSshAtom(value: string, maxLength: number): boolean {
  const trimmed = value.trim()
  return trimmed.length > 0 && trimmed.length <= maxLength && !trimmed.startsWith('-') && !/[\s\u0000-\u001f\u007f]/u.test(trimmed)
}

function safeForwardAtom(value: string): boolean {
  const trimmed = value.trim()
  return safeSshAtom(trimmed, 512) && !trimmed.includes(',')
}

function validPort(value: number): boolean {
  return Number.isInteger(value) && value >= 1 && value <= 65_535
}
