import { randomUUID } from 'node:crypto'
import { copyFile, mkdir, open, readFile, rename, rm } from 'node:fs/promises'
import { dirname } from 'node:path'
import type { ZodType } from 'zod'

export class AtomicJsonStore<T> {
  readonly #filePath: string
  readonly #schema: ZodType<T>
  readonly #defaults: T
  #writeChain: Promise<void> = Promise.resolve()

  constructor(filePath: string, schema: ZodType<T>, defaults: T) {
    this.#filePath = filePath
    this.#schema = schema
    this.#defaults = structuredClone(defaults)
  }

  async load(): Promise<T> {
    try {
      const raw = await readFile(this.#filePath, 'utf8')
      return this.#schema.parse(JSON.parse(raw) as unknown)
    } catch (error) {
      if (isMissingFile(error)) {
        const initial = this.#schema.parse(structuredClone(this.#defaults))
        await this.save(initial)
        return initial
      }
      throw error
    }
  }

  async save(value: T): Promise<T> {
    const validated = this.#schema.parse(value)
    const pending = this.#writeChain.then(() => this.#write(validated))
    this.#writeChain = pending.catch(() => undefined)
    await pending
    return structuredClone(validated)
  }

  async update(mutator: (current: T) => T | Promise<T>): Promise<T> {
    let result: T | undefined
    const pending = this.#writeChain.then(async () => {
      const current = await this.loadWithoutQueue()
      result = this.#schema.parse(await mutator(structuredClone(current)))
      await this.#write(result)
    })
    this.#writeChain = pending.catch(() => undefined)
    await pending
    if (result === undefined) throw new Error('Atomic update completed without a result')
    return structuredClone(result)
  }

  private async loadWithoutQueue(): Promise<T> {
    try {
      const raw = await readFile(this.#filePath, 'utf8')
      return this.#schema.parse(JSON.parse(raw) as unknown)
    } catch (error) {
      if (isMissingFile(error)) return this.#schema.parse(structuredClone(this.#defaults))
      throw error
    }
  }

  async #write(value: T): Promise<void> {
    const directory = dirname(this.#filePath)
    const temporaryPath = `${this.#filePath}.${String(process.pid)}.${randomUUID()}.tmp`
    const backupPath = `${this.#filePath}.bak`
    await mkdir(directory, { recursive: true })
    try {
      const handle = await open(temporaryPath, 'wx', 0o600)
      try {
        await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`, 'utf8')
        await handle.sync()
      } finally {
        await handle.close()
      }
      try {
        await copyFile(this.#filePath, backupPath)
      } catch (error) {
        if (!isMissingFile(error)) throw error
      }
      await rename(temporaryPath, this.#filePath)
    } finally {
      await rm(temporaryPath, { force: true })
    }
  }
}

function isMissingFile(error: unknown): boolean {
  return error instanceof Error && 'code' in error && error.code === 'ENOENT'
}
