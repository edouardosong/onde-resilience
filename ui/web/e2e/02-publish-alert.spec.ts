import { test, expect } from '@playwright/test';

/**
 * 02 — Publication d'une alerte
 * Plan legacy : testsprite/02-publish-alert.plan.json (p0)
 *   « L'utilisateur publie une alerte critique ; elle apparaît dans le
 *     feed et est diffusée. »
 *
 * Comportement réel (ui/src/index.html, mode démo — sans Tauri) :
 *   - le formulaire de publication est la barre `#compose-bar` visible sur
 *     le flux ; `#btn-alerts` met le mode de publication sur 'alert' ;
 *   - à l'envoi, l'app insère en tête du flux une carte « Moi / À l'instant
 *     / ALERTE / ✓ PoW calculé · 0 hops · diffusion en cours... » et vide
 *     le champ. C'est la « confirmation de publication » du mode démo.
 *
 * GAP (non testable tel quel, documenté) :
 *   - « événement signé, ID visible » : en mode démo l'app ne produit ni
 *     signature réelle ni ID d'événement (le badge affiché est « PoW calculé
 *     » simulé). En mode Tauri la commande réelle est `publish_alert`
 *     (ui/src-tauri/src/commands.rs) — inaccessible depuis un navigateur
 *     Playwright sans binaire Tauri ;
 *   - « zone géographique » : le formulaire de l'UI n'a pas de champ zone
 *     (seul le texte est saisi).
 */
test.describe('02 — Publication d\'une alerte critique', () => {
  test('l\'alerte saisie est publiée et visible en tête du flux', async ({ page }) => {
    const message = `Coupure d'eau secteur 7 — point d'eau place de la Mairie [e2e-${Date.now()}]`;

    await page.goto('/');
    // Le flux est la page active par défaut ; on est bien en mode « Alertes ».
    await expect(page.locator('#page-feed')).toHaveClass(/active/);
    await page.locator('#btn-alerts').click();
    await expect(page.locator('#btn-alerts')).toHaveCSS('background-color', 'rgb(0, 255, 136)');

    const before = await page.locator('#feed-container .card').count();

    // Saisie de l'alerte critique (message du plan legacy) + validation.
    await page.locator('#compose-input').fill(message);
    await page.locator('#compose-send').click();

    // 1) Confirmation de publication du mode démo : la carte apparaît dans
    //    le flux avec le contenu exact, le type ALERTE et le badge PoW
    //    simulé (« diffusion en cours »).
    const card = page.locator('#feed-container .card').filter({ hasText: message });
    await expect(card).toHaveCount(1);
    await expect(card).toContainText('ALERTE');
    await expect(card).toContainText('Moi');
    await expect(card).toContainText(/PoW calculé/);
    await expect(card).toContainText(/diffusion en cours/);

    // 2) L'alerte est en tête du flux (insertion avant la première carte),
    //    le flux a gagné exactement une carte, et le champ est vidé.
    await expect(page.locator('#feed-container .card')).toHaveCount(before + 1);
    await expect(page.locator('#feed-container .card').first()).toContainText(message);
    await expect(page.locator('#compose-input')).toHaveValue('');
  });
});
