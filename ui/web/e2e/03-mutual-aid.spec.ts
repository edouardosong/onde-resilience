import { test, expect } from '@playwright/test';

/**
 * 03 — Publication d'une demande d'entraide
 * Plan legacy : testsprite/03-mutual-aid.plan.json (p0)
 *   « L'utilisateur publie une demande d'entraide (besoin) ; elle est
 *     diffusée et visible par les pairs. »
 *
 * Comportement réel (ui/src/index.html, mode démo) :
 *   - `#btn-aid` bascule le MODE de publication sur 'entraide' (feedMode),
 *     visuellement confirmé par le style actif du bouton ;
 *   - à l'envoi, la carte « Moi / À l'instant / ✓ PoW calculé · 0 hops ·
 *     diffusion en cours... » est insérée en tête du flux.
 *
 * ÉCARTS HONNÊTES (documentés — ne pas forcer de test faux) :
 *   - Comportement de l'app en mode démo : le badge de type de la carte
 *     publiée est HARDCODÉ « ALERTE » quel que soit le mode de publication
 *     (ui/src/index.html, bloc « Mode démo »). On n'assert donc PAS un
 *     badge « ENTRIADE » — la preuve d'entraide est le mode actif du
 *     formulaire + l'insertion de la carte.
 *   - Le plan legacy attend « filtrer le flux par type entraide » : le flux
 *     affisé n'est PAS filtré par les boutons (ils ne règlent que le mode
 *     de publication) — voir 04-feed.spec.ts.
 *   - « contact optionnel » : aucun champ contact dans le formulaire.
 *   - En mode Tauri, la commande réelle serait `publish_mutual_aid` —
 *     inaccessible sans binaire Tauri (mode navigateur uniquement ici).
 */
test.describe('03 — Publication d\'une demande d\'entraide', () => {
  test('la demande d\'entraide est publiée en mode entraide et visible dans le flux', async ({ page }) => {
    const message = `Recherche générateur électrique — secteur sud [e2e-${Date.now()}]`;

    await page.goto('/');
    await expect(page.locator('#page-feed')).toHaveClass(/active/);

    // Bascule du formulaire en mode « Entraide » (état vérifiable : style actif).
    await page.locator('#btn-aid').click();
    await expect(page.locator('#btn-aid')).toHaveCSS('background-color', 'rgb(0, 255, 136)');
    // Et le bouton « Alertes » repasse en état inactif.
    await expect(page.locator('#btn-alerts')).not.toHaveCSS('background-color', 'rgb(0, 255, 136)');

    const before = await page.locator('#feed-container .card').count();

    // Saisie de la demande d'entraide (message du plan legacy) + validation.
    await page.locator('#compose-input').fill(message);
    await page.locator('#compose-send').click();

    // La demande est publiée : carte présente dans le flux avec le contenu
    // exact, l'auteur « Moi » et le badge de diffusion du mode démo.
    const card = page.locator('#feed-container .card').filter({ hasText: message });
    await expect(card).toHaveCount(1);
    await expect(card).toContainText('Moi');
    await expect(card).toContainText(/PoW calculé/);
    await expect(card).toContainText(/diffusion en cours/);

    // Insertion en tête + comptage relatif (robuste aux cartes de démo
    // insérées automatiquement toutes les 30 s par l'app).
    await expect(page.locator('#feed-container .card')).toHaveCount(before + 1);
    await expect(page.locator('#feed-container .card').first()).toContainText(message);
    await expect(page.locator('#compose-input')).toHaveValue('');
  });
});
