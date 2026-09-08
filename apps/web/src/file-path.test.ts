import { describe, expect, it } from 'vitest'
import { breadcrumbs } from './panels/FilePanel'

describe('SFTP breadcrumbs', () => {
  it('keeps the remote home shortcut intact', () => {
    expect(breadcrumbs('~')).toEqual([{ label: '~', path: '~' }])
  })

  it('builds absolute Linux paths without losing the root', () => {
    expect(breadcrumbs('/srv/project/data')).toEqual([
      { label: 'srv', path: '/srv' },
      { label: 'project', path: '/srv/project' },
      { label: 'data', path: '/srv/project/data' }
    ])
  })

  it('builds relative remote paths', () => {
    expect(breadcrumbs('project/output')).toEqual([
      { label: 'project', path: 'project' },
      { label: 'output', path: 'project/output' }
    ])
  })
})
