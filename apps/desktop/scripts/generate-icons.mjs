import { deflateSync } from 'node:zlib'
import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const outputDirectory = join(import.meta.dirname, '..', 'build')
const pngSignature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])
mkdirSync(outputDirectory, { recursive: true })
const sizes = [16, 24, 32, 48, 64, 128, 256]
const images = sizes.map((size) => ({ size, png: createIconPng(size) }))
writeFileSync(join(outputDirectory, 'icon.png'), images.at(-1).png)
writeFileSync(join(outputDirectory, 'icon.ico'), createIco(images))

function createIconPng(size) {
  const pixels = Buffer.alloc((size * 4 + 1) * size)
  const radius = size * 0.19
  for (let y = 0; y < size; y += 1) {
    const row = y * (size * 4 + 1)
    pixels[row] = 0
    for (let x = 0; x < size; x += 1) {
      const offset = row + 1 + x * 4
      const inside = roundedSquare(x, y, size, radius)
      if (!inside) continue
      const blend = (x + y) / (size * 2)
      pixels[offset] = Math.round(247 - blend * 45)
      pixels[offset + 1] = Math.round(129 - blend * 38)
      pixels[offset + 2] = Math.round(47 + blend * 105)
      pixels[offset + 3] = 255
      const normalizedX = x / size
      const normalizedY = y / size
      const stroke = Math.max(0.035, 1.2 / size)
      const chevron = normalizedX >= 0.25 && normalizedX <= 0.53 && (
        Math.abs(normalizedY - (0.12 + normalizedX * 0.72)) < stroke ||
        Math.abs(normalizedY - (0.88 - normalizedX * 0.72)) < stroke
      )
      const cursor = normalizedX >= 0.53 && normalizedX <= 0.76 && normalizedY >= 0.67 && normalizedY <= 0.67 + stroke * 1.8
      if (chevron || cursor) {
        pixels[offset] = 255
        pixels[offset + 1] = 255
        pixels[offset + 2] = 255
      }
    }
  }
  const header = Buffer.alloc(13)
  header.writeUInt32BE(size, 0)
  header.writeUInt32BE(size, 4)
  header[8] = 8
  header[9] = 6
  return Buffer.concat([pngSignature, chunk('IHDR', header), chunk('IDAT', deflateSync(pixels, { level: 9 })), chunk('IEND', Buffer.alloc(0))])
}

function roundedSquare(x, y, size, radius) {
  const margin = size * 0.045
  const left = margin
  const right = size - margin - 1
  const top = margin
  const bottom = size - margin - 1
  const nearX = Math.max(left + radius, Math.min(x, right - radius))
  const nearY = Math.max(top + radius, Math.min(y, bottom - radius))
  return (x - nearX) ** 2 + (y - nearY) ** 2 <= radius ** 2
}

function createIco(images) {
  const header = Buffer.alloc(6)
  header.writeUInt16LE(0, 0)
  header.writeUInt16LE(1, 2)
  header.writeUInt16LE(images.length, 4)
  const directory = Buffer.alloc(images.length * 16)
  let offset = header.byteLength + directory.byteLength
  images.forEach(({ size, png }, index) => {
    const entry = index * 16
    directory[entry] = size === 256 ? 0 : size
    directory[entry + 1] = size === 256 ? 0 : size
    directory.writeUInt16LE(1, entry + 4)
    directory.writeUInt16LE(32, entry + 6)
    directory.writeUInt32LE(png.byteLength, entry + 8)
    directory.writeUInt32LE(offset, entry + 12)
    offset += png.byteLength
  })
  return Buffer.concat([header, directory, ...images.map(({ png }) => png)])
}

function chunk(name, contents) {
  const type = Buffer.from(name, 'ascii')
  const length = Buffer.alloc(4)
  length.writeUInt32BE(contents.byteLength)
  const checksum = Buffer.alloc(4)
  checksum.writeUInt32BE(crc32(Buffer.concat([type, contents])))
  return Buffer.concat([length, type, contents, checksum])
}

function crc32(contents) {
  let crc = 0xffffffff
  for (const byte of contents) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  return (crc ^ 0xffffffff) >>> 0
}
