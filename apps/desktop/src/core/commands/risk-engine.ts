import type { CommandAnalysis } from '../../protocol/command'
import type { CommandPreset } from '../../protocol/domain'

const order = { L0: 0, L1: 1, L2: 2 } as const
const levelTwoPatterns: Array<[RegExp, string]> = [
  [/\b(?:reboot|shutdown|poweroff|halt)\b/i, '包含关机或重启操作'],
  [/\b(?:mkfs(?:\.[a-z0-9]+)?|fdisk|parted|wipefs)\b/i, '包含磁盘或文件系统修改'],
  [/\brm\s+(?:-[^\s]*r[^\s]*f|-[^\s]*f[^\s]*r)\b/i, '包含递归强制删除'],
  [/\bdd\s+[^\n]*(?:\bof=|\bif=)/i, '包含原始块复制'],
  [/\bgit\s+(?:reset\s+--hard|clean\s+-[^\s]*f|push\s+[^\n]*--force)\b/i, '包含破坏性 Git 操作']
]
const levelOnePatterns: Array<[RegExp, string]> = [
  [/\bsudo\b/i, '请求提升权限'],
  [/\b(?:rm|mv|cp|mkdir|rmdir|touch|chmod|chown|kill|pkill|killall)\b/i, '可能修改文件或进程状态'],
  [/\b(?:systemctl|service)\s+(?:start|stop|restart|reload|enable|disable)\b/i, '修改服务状态'],
  [/\b(?:apt|apt-get|dnf|yum|pacman|zypper|snap|npm|pnpm|pip)\s+(?:install|remove|upgrade|update|add)\b/i, '修改已安装软件'],
  [/\bgit\s+(?:add|commit|checkout|switch|merge|rebase|pull|push|restore|tag)\b/i, '修改 Git 工作区或远端'],
  [/\bcurl\b[^\n|]*\|\s*(?:sh|bash)\b/i, '下载并执行远程脚本'],
  [/(?:^|[^<])(?:>>?|2>|&>)/m, '包含输出重定向，可能修改文件'],
  [/\b(?:tee|truncate)\b|\bsed\s+-[^\s]*i\b/i, '包含文件写入工具或原地编辑']
]

const readOnlyCommands = new Set([
  'awk', 'btop', 'cat', 'cut', 'date', 'df', 'du', 'echo', 'env', 'find', 'free', 'git', 'grep', 'head',
  'hostname', 'id', 'ip', 'journalctl', 'ls', 'lscpu', 'lsblk', 'nvidia-smi', 'printf', 'ps', 'pwd', 'sed',
  'squeue', 'ss', 'stat', 'tail', 'top', 'uname', 'uptime', 'vmstat', 'wc', 'who', 'whoami'
])

export function analyzeCommandRisk(command: string, declaredRisk: CommandPreset['risk'], requiredConfirmation: string | null): CommandAnalysis {
  const reasons: string[] = []
  let detectedRisk: CommandPreset['risk'] = isReadOnlyCommand(command) ? 'L0' : 'L1'
  if (detectedRisk === 'L1') reasons.push('自定义命令不在只读允许集内，按状态变更处理')
  for (const [pattern, reason] of levelOnePatterns) {
    if (pattern.test(command)) { detectedRisk = maxRisk(detectedRisk, 'L1'); reasons.push(reason) }
  }
  for (const [pattern, reason] of levelTwoPatterns) {
    if (pattern.test(command)) { detectedRisk = 'L2'; reasons.push(reason) }
  }
  if (order[declaredRisk] > order[detectedRisk]) reasons.push(`预设声明风险为 ${declaredRisk}`)
  const effectiveRisk = maxRisk(declaredRisk, detectedRisk)
  return { declaredRisk, detectedRisk, effectiveRisk, reasons: [...new Set(reasons)], requiredConfirmation: effectiveRisk === 'L2' ? requiredConfirmation : null }
}

function isReadOnlyCommand(command: string): boolean {
  const segments = command
    .replaceAll(/\$\([^)]*\)/g, '')
    .split(/(?:\n|&&|\|\||;|\|)/)
    .map((segment) => segment.trim())
    .filter(Boolean)
  if (!segments.length) return false
  return segments.every((segment) => {
    const normalized = segment.replace(/^(?:[A-Za-z_][A-Za-z0-9_]*=[^\s]+\s+)*/, '').replace(/^command\s+/, '')
    const match = normalized.match(/^(?:if\s+)?(?:exec\s+)?([A-Za-z0-9_.-]+)/)
    if (!match?.[1]) return /^(?:then|else|fi)$/.test(normalized)
    return readOnlyCommands.has(match[1]) && !(match[1] === 'git' && !/^git\s+(?:status|branch|log|diff|show|rev-parse)\b/.test(normalized))
  })
}

function maxRisk(left: CommandPreset['risk'], right: CommandPreset['risk']): CommandPreset['risk'] {
  return order[left] >= order[right] ? left : right
}
