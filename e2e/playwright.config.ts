import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  timeout: 300_000,          // LLM local : chaque ai* = round-trip screenshot+inférence
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    headless: true,
    viewport: { width: 480, height: 960 },   // format téléphone comme sur appareil réel
    actionTimeout: 30_000,
  },
});
