import { describe, expect, it } from 'vitest'
import { analyzeCommandRisk } from './risk-engine'

describe('command risk analysis', () => {
  it('keeps known read-only commands at L0 and never lowers declared risk', () => {
    expect(analyzeCommandRisk('git status --short --branch', 'L0', 'worker')).toMatchObject({ detectedRisk: 'L0', effectiveRisk: 'L0' })
    expect(analyzeCommandRisk('uptime', 'L2', 'TYPE-ME')).toMatchObject({ detectedRisk: 'L0', effectiveRisk: 'L2', requiredConfirmation: 'TYPE-ME' })
  })

  it('classifies state changes and destructive commands conservatively', () => {
    expect(analyzeCommandRisk('systemctl restart nginx', 'L0', 'worker')).toMatchObject({ detectedRisk: 'L1', effectiveRisk: 'L1' })
    expect(analyzeCommandRisk('git reset --hard HEAD~1', 'L0', 'worker')).toMatchObject({ detectedRisk: 'L2', effectiveRisk: 'L2', requiredConfirmation: 'worker' })
    expect(analyzeCommandRisk('my-company-tool deploy', 'L0', 'worker')).toMatchObject({ detectedRisk: 'L1', effectiveRisk: 'L1' })
  })
})
