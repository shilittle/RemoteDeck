import { resolve } from 'node:path'
import react from '@vitejs/plugin-react'
import { defineConfig, loadEnv } from 'vite'

export default defineConfig(({ mode }) => {
  const webRoot = import.meta.dirname
  const env = loadEnv(mode, webRoot, '')
  const backend = env.REMOTEDECK_BACKEND_URL
  return {
    root: webRoot,
    plugins: [react()],
    server: {
      host: '127.0.0.1', port: 1420, strictPort: true,
      proxy: backend ? {
        '/api': { target: backend, changeOrigin: true, ws: true }
      } : undefined
    },
    build: {
      outDir: resolve(webRoot, 'dist'), emptyOutDir: true, sourcemap: false,
      rollupOptions: { output: { manualChunks: { react: ['react', 'react-dom', 'zustand'], terminal: ['@xterm/xterm', '@xterm/addon-fit', '@xterm/addon-search', '@xterm/addon-unicode11'] } } }
    }
  }
})
