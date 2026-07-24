import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'electron-vite'

export default defineConfig({
  main: {
    build: { externalizeDeps: true, rollupOptions: { input: resolve('src/main/index.ts') } }
  },
  preload: {
    build: {
      externalizeDeps: false,
      rollupOptions: {
        input: resolve('src/preload/index.ts'),
        output: { format: 'cjs', entryFileNames: '[name].cjs', inlineDynamicImports: true }
      }
    }
  },
  renderer: {
    root: resolve('src/renderer'),
    plugins: [react()]
  }
})
