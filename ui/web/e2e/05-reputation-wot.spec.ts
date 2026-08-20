import { test, expect } from '@playwright/test';

/**
 * 05 — Réputation / Web of Trust
 * Plan legacy : testsprite/05-reputation-wot.plan.json (p1)
 *   « L'écran de réputation affiche la confiance par pair, les endossements,
 *     et reflète les nœuds de confiance vs inconnus. »
 *
 * CONSTAT (vérifié dans le code, pas supposé) :
 *   - `ui/src/index.html` ne contient AUCUN écran réputation/WoT :
 *     grep `reputation|confiance|endosse|wot` → 0 occurrence. Les 6 onglets
 *     sont Alertes / Radar / IA / Wallet / Wiki / P2P.
 *   - Le backend Tauri expose la commande `get_reputation`
 *     (ui/src-tauri/src/commands.rs, ligne 142), mais l'UI web ne l'appelle
 *     JAMAIS (les seuls invokes UI : node_start, get_feed_events,
 *     publish_alert, publish_mutual_aid).
 *   - Playwright teste l'UI en mode navigateur (démo, sans Tauri) : même si
 *     un écran WoT existait, il faudrait l'exposer côté web.
 *
 * HONNÊTETÉ (règle L2-06 §5 — ne PAS forcer un test faux) :
 *   - Le scénario complet (liste de pairs, scores, endossements, PoW
 *     adaptatif) est donc marqué SKIP, avec la raison exacte ;
 *   - Le test LIVE ci-dessous couvre la surface réelle la plus proche :
 *     l'état du mesh / du nœud (stats Radar + badge mesh) — la donnée que
 *     l'écran WoT consommera, afin qu'une régression sur cet état soit
 *     détectée dès aujourd'hui.
 */
test.describe('05 — Réputation / Web of Trust', () => {
  test('baseline : l\'état du nœud et du mesh est exposé (surface réelle la plus proche)', async ({ page }) => {
    await page.goto('/');

    // Identité / état global du nœud (header).
    await expect(page.locator('.logo')).toHaveText(/ONDE/);
    await expect(page.locator('#mesh-badge')).toHaveText(/Mesh/);
    await expect(page.locator('#status-dot')).toBeVisible();

    // État du mesh (page Radar) : la UI expose aujourd'hui des agrégats de
    // nœuds (proximité, ponts, buffer DTN, uptime, position geohash) — pas
    // de pair individuel, pas de score de confiance.
    await page.locator('.tab[data-page="page-radar"]').click();
    await expect(page.locator('#page-radar')).toHaveClass(/active/);
    await expect(page.locator('#node-count')).toHaveText(/\d+/);
    await expect(page.locator('#bridge-count')).toHaveText(/\d+/);
    await expect(page.locator('#dtn-buffer')).toHaveText(/\d+/);
    await expect(page.locator('#uptime-label')).toHaveText(/.+/);
    await expect(page.locator('#geohash-value')).toHaveText(/^[a-z0-9]{5,8}$/);  // valeur de démo « u09tunq » (non base32 stricte)
    await expect(page.locator('.radar-container .node-dot').first()).toBeVisible();
  });

  test.skip(
    'BLOQUÉ — composant absent de l\'UI : aucun écran Réputation/WoT dans ' +
    'ui/src/index.html (0 onglet, 0 référence « confiance/endossement »). ' +
    'La commande Tauri get_reputation existe (ui/src-tauri/src/commands.rs:142) ' +
    'mais n\'est jamais appelée par l\'UI web, et Playwright ne teste que le ' +
    'mode navigateur (sans Tauri). Ré-activer dès qu\'un écran WoT sera exposé ' +
    'côté web (ROADMAP 1.2 « Propagation WoT » / 2.7 « Réputation anti-abus ») : ' +
    'listes de pairs + scores, détail pair (endossements, PoW adaptatif), ' +
    'bouton Endosser avec confirmation de diffusion.',
    async ({ page }) => {
      await page.goto('/');
      // À implémenter quand l'écran existera :
      //   - ouvrir l'écran Réputation/WoT ;
      //   - lister les pairs avec identité + score de confiance + statut ;
      //   - ouvrir le détail d'un pair (endossements, PoW, dernière activité) ;
      //   - endosser un pair → niveau supérieur + événement marqué diffusé.
      expect(page.locator('#page-wot')).toBeVisible();
    }
  );
});
