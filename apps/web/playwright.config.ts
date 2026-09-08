import { defineConfig } from '@playwright/test'

export default defineConfig({
  testDir: './e2e',
  globalSetup: './e2e/global-setup.ts',
  timeout: 45_000,
  workers: 1,
  fullyParallel: false,
  use: { baseURL: 'http://127.0.0.1:1420', actionTimeout: 15_000, trace: 'retain-on-failure', channel: process.platform === 'win32' ? 'msedge' : undefined }
})
