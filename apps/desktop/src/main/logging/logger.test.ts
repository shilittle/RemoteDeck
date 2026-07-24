import { mkdtemp, readdir, rm, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { pruneLogFiles } from './logger'

const temporaryDirectories: string[] = []

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })))
})

describe('application log retention', () => {
  it('keeps only the newest bounded set of owned log files', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-logs-'))
    temporaryDirectories.push(directory)
    for (let index = 0; index < 6; index += 1) {
      const path = join(directory, `remotedeck-${String(index)}.log`)
      await writeFile(path, String(index))
      const time = new Date(1_700_000_000_000 + index * 1000)
      await utimes(path, time, time)
    }
    await writeFile(join(directory, 'foreign.log'), 'preserve')
    pruneLogFiles(directory, 3)
    expect((await readdir(directory)).sort()).toEqual(['foreign.log', 'remotedeck-3.log', 'remotedeck-4.log', 'remotedeck-5.log'])
  })
})
