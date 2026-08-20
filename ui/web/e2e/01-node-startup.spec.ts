import { test, expect } from '@playwright/test';

/**
 * 01 — Démarrage du nœud
 * Plan legacy : testsprite/01-node-startup.plan.json (p0)
 *   « Le nœud démarre, affiche son identité et son état de santé
 *     (transport, réputation, stockage). »
 *
 * Ce que l'UI réelle expose (ui/src/index.html, mode démo) :
 *   - identité : logo ONDE + badge mesh (`#mesh-badge`) + voyant (`#status-dot`)
 *   - app shell : 6 onglets de navigation (Alertes/Radar/IA/Wallet/Wiki/P2P)
 *   - état de santé : page Radar (nœuds proches, ponts, DTN, uptime, geohash)
 *
 * GAP (non testable tel quel, documenté) :
 *   - « réputation globale » : aucun écran réputation/WoT dans l'UI
 *     (voir 05-reputation-wot.spec.ts) ;
 *   - « panneau des logs/santé du nœud » (logs de démarrage, identité
 *     générée, SQLite ouverte) : le composant n'existe pas dans l'UI web —
 *     cet état n'est exposé qu'en mode Tauri (node_status), hors de portée
 *     d'un test navigateur.
 */
test.describe('01 — Démarrage du nœud', () => {
  test('l\'app démarre, affiche son identité et son état de santé', async ({ page }) => {
    await page.goto('/');

    // 1) L\'écran principal s\'affiche avec l\'identité du nœud (logo ONDE)
    //    et un indicateur d\'état (badge mesh + voyant).
    await expect(page.locator('.logo')).toHaveText(/ONDE/);
    const badge = page.locator('#mesh-badge');
    await expect(badge).toBeVisible();
    // Le texte bascule aléatoirement « Mesh Actif » ↔ « Mesh faible » (timer démo,
    // 1 tirage / 5 s) — on n\'assert donc que le statut mesh est affiché.
    await expect(badge).toHaveText(/Mesh/);
    await expect(page.locator('#status-dot')).toBeVisible();

    // 2) App shell complet : les 6 onglets de navigation sont présents.
    for (const pageId of ['page-feed', 'page-radar', 'page-ai', 'page-wallet', 'page-encyclopedia', 'page-p2p']) {
      await expect(page.locator(`.tab[data-page="${pageId}"]`)).toBeVisible();
      await expect(page.locator(`#${pageId}`)).toBeAttached();
    }
    // L\'onglet actif initial est le flux (feed).
    await expect(page.locator('#page-feed')).toHaveClass(/active/);

    // 3) État de santé : la page Radar expose les stats du nœud
    //    (transport/proximité, stockage DTN, uptime) — les valeurs de démo.
    await page.locator('.tab[data-page="page-radar"]').click();
    await expect(page.locator('#page-radar')).toHaveClass(/active/);
    await expect(page.locator('#node-count')).toHaveText(/\d+/);
    await expect(page.locator('#bridge-count')).toHaveText(/\d+/);
    await expect(page.locator('#dtn-buffer')).toHaveText(/\d+/);
    await expect(page.locator('#uptime-label')).toHaveText(/.+/);
    await expect(page.locator('#geohash-value')).toHaveText(/^[a-z0-9]{5,8}$/);  // valeur de démo « u09tunq » (non base32 stricte)
  });
});
