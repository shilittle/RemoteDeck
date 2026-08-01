import { describe, expect, it } from 'vitest'
import { confirmationMatches, maxRisk, riskLabel } from './risk'

describe('command risk helpers', () => {
  it('never lowers the effective risk', () => {
    expect(maxRisk('L0', 'L2')).toBe('L2')
    expect(maxRisk('L1', 'L0')).toBe('L1')
  })

  it('requires an exact backend-provided L2 confirmation', () => {
    const analysis = { effectiveRisk: 'L2' as const, requiredConfirmation: 'gpu-lab' }
    expect(confirmationMatches(analysis, 'gpu-lab')).toBe(true)
    expect(confirmationMatches(analysis, 'GPU-LAB')).toBe(false)
    expect(confirmationMatches({ effectiveRisk: 'L2', requiredConfirmation: null }, '')).toBe(false)
  })

  it('lets an explicit L1 dialog confirmation use the backend token', () => {
    expect(confirmationMatches({ effectiveRisk: 'L1', requiredConfirmation: 'gpu-lab' }, '')).toBe(true)
  })

  it('labels every risk level', () => {
    expect(['L0', 'L1', 'L2'].map((risk) => riskLabel(risk as 'L0' | 'L1' | 'L2'))).toEqual(['只读', '修改状态', '高风险'])
  })
})
