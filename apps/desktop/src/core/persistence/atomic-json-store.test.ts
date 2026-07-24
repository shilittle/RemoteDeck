import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { z } from 'zod'
import { AtomicJsonStore } from './atomic-json-store'

const directories: string[] = []
const schema = z.object({ schemaVersion: z.literal(1), counter: z.number().int().nonnegative() })

afterEach(async () => {
  await Promise.all(directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })))
})

describe('AtomicJsonStore', () => {
  it('creates defaults and persists serialized updates', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-store-'))
    directories.push(directory)
    const file = join(directory, 'settings.json')
    const store = new AtomicJsonStore(file, schema, { schemaVersion: 1, counter: 0 })
    expect(await store.load()).toEqual({ schemaVersion: 1, counter: 0 })
    await Promise.all(Array.from({ length: 8 }, () => store.update((value) => ({ ...value, counter: value.counter + 1 }))))
    expect(await store.load()).toEqual({ schemaVersion: 1, counter: 8 })
    expect(JSON.parse(await readFile(file, 'utf8'))).toEqual({ schemaVersion: 1, counter: 8 })
  })

  it('hard-fails invalid persisted data instead of using it', async () => {
    const directory = await mkdtemp(join(tmpdir(), 'remotedeck-store-'))
    directories.push(directory)
    const file = join(directory, 'settings.json')
    await writeFile(file, '{"schemaVersion":1,"counter":-1}', 'utf8')
    const store = new AtomicJsonStore(file, schema, { schemaVersion: 1, counter: 0 })
    await expect(store.load()).rejects.toThrow()
  })
})

