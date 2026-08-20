import { defineConfig, devices } from '@playwright/test';

/**
 * ONDE — L2-06 : baseline E2E UI (Playwright — 100% gratuit / open source,
 * remplace TestSprite par décision utilisateur 2026-08-20).
 *
 * Cible : l'UI de l'app ONDE (`ui/src/index.html`, servie par le dev server
 * Vite de ce package — `vite.config.js` définit `root: '../src'`). Les tests
 * s'exécutent en mode DÉMO (navigateur, sans Tauri) : c'est le seul mode
 * accessible depuis Playwright sans binaire Tauri. Les écarts avec les plans
 * legacy `testsprite/*.plan.json` sont documentés spec par spec (commentaires
 * "GAP") et dans `e2e/README.md`.
 *
 * Arbitrages (justifiés) :
 * - port 5173 fixe + `--strictPort` : URL déterministe pour le webServer
 *   (détecter le port est possible, mais un port strict évite toute
 *   collision avec un serveur déjà lancé et rend les logs reproductibles) ;
 * - `workers: 1` : un seul dev server Vite partagé, preuves ordonnées,
 *   aucune contention sur les timers de démo de l'app ;
 * - `retries: 1` : tolérance aux micro-flakiness (l'app démo insère des
 *   cartes toutes les 30 s et fait varier le badge mesh toutes les 5 s) ;
 * - `screenshot: 'on'` + `trace: 'on'` : exigence L2-06 §3 — chaque test
 *   capture son évidence (captures d'action + trace zip) dans `e2e-results/` ;
 * - rapport `html` (open: never) + `list` : lisible en CI et navigable localement.
 */
export default defineConfig({
  testDir: './e2e',
  outputDir: './e2e-results',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 1,
  workers: 1,
  timeout: 30_000,
  expect: { timeout: 10_000 },
  reporter: [
    ['list'],
    ['html', { open: 'never', outputFolder: './playwright-report' }],
  ],
  use: {
    baseURL: 'http://127.0.0.1:5173',
    headless: true,
    screenshot: 'on',
    trace: 'on',
    actionTimeout: 10_000,
    navigationTimeout: 15_000,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: {
    command: 'npm run dev -- --port 5173 --strictPort --host 127.0.0.1',
    url: 'http://127.0.0.1:5173',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
