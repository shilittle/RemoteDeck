import type { CommandAnalysis, CommandRisk } from './types'

const ranks: Record<CommandRisk, number> = { L0: 0, L1: 1, L2: 2 }

export function maxRisk(left: CommandRisk, right: CommandRisk): CommandRisk {
  return ranks[left] >= ranks[right] ? left : right
}

export function riskLabel(risk: CommandRisk): string {
  return { L0: '只读', L1: '修改状态', L2: '高风险' }[risk]
}

export function confirmationMatches(analysis: Pick<CommandAnalysis, 'effectiveRisk' | 'requiredConfirmation'>, value: string): boolean {
  if (analysis.effectiveRisk !== 'L2') return true
  return Boolean(analysis.requiredConfirmation) && value === analysis.requiredConfirmation
}
