import { test, expect } from '@playwright/test';

/**
 * 07 — Redit: communautés, publication et navigation sociale
 *
 * Mode démo (sans Tauri) — le comportement vérifié est celui du frontend
 * web statique.
 */
test.describe('07 — Redit: communautés et flux social', () => {
  test('le mode Redit est accessible et affiche les communautés', async ({ page }) => {
    await page.goto('/');

    // Basculer en mode Redit
    await page.locator('.mode-btn[data-mode="redit"]').click();
    await expect(page.locator('#redit-feed')).toHaveClass(/active/);

    // La barre de composition Redit est visible (titre + contenu)
    await expect(page.locator('#compose-bar-redit')).toBeVisible();
    // La nav Redit est visible
    await expect(page.locator('#nav-redit')).toBeVisible();

    // Le flux Redit montre l'état vide initial
    await expect(page.locator('#redit-feed-container .empty-state')).toBeVisible();
  });

  test('navigation vers les communautés Redit', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="redit"]').click();

    // Navigation vers les communautés
    await page.locator('#nav-redit .tab[data-page="redit-communities"]').click();
    await expect(page.locator('#redit-communities')).toHaveClass(/active/);

    // Les communautés par défaut sont affichées
    await expect(page.locator('#redit-community-list .card')).toHaveCount(3);
    await expect(page.locator('#redit-community-list')).toContainText('r/entraide');
    await expect(page.locator('#redit-community-list')).toContainText('r/tech');
  });

  test('publication simulée Redit en mode démo', async ({ page }) => {
    const title = `Discussion test e2e — ${Date.now()}`;
    const body = 'Ceci est une discussion simulée pour le test E2E.';

    await page.goto('/');
    await page.locator('.mode-btn[data-mode="redit"]').click();

    await page.locator('#redit-title-input').fill(title);
    await page.locator('#redit-compose-input').fill(body);
    await page.locator('#redit-compose-send').click();

    // La carte simulée apparaît dans le flux
    await expect(page.locator('#redit-feed-container .card').filter({ hasText: title })).toHaveCount(1);
    await expect(page.locator('#redit-feed-container').filter({ hasText: body })).toHaveCount(1);

    // Les champs sont vidés
    await expect(page.locator('#redit-title-input')).toHaveValue('');
    await expect(page.locator('#redit-compose-input')).toHaveValue('');
  });

  test('navigation entre les onglets Redit', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="redit"]').click();

    // Recherche
    await page.locator('#nav-redit .tab[data-page="redit-search"]').click();
    await expect(page.locator('#redit-search')).toHaveClass(/active/);

    // Messages
    await page.locator('#nav-redit .tab[data-page="redit-messages"]').click();
    await expect(page.locator('#redit-messages')).toHaveClass(/active/);

    // Profil
    await page.locator('#nav-redit .tab[data-page="redit-profile"]').click();
    await expect(page.locator('#redit-profile')).toHaveClass(/active/);
    await expect(page.locator('#redit-display-name')).toBeVisible();

    // Retour au flux
    await page.locator('#nav-redit .tab[data-page="redit-feed"]').click();
    await expect(page.locator('#redit-feed')).toHaveClass(/active/);
  });

  test('champ de jointure de communauté visible', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="redit"]').click();
    await page.locator('#nav-redit .tab[data-page="redit-communities"]').click();

    // Le champ et le bouton de jointure sont présents
    await expect(page.locator('#redit-community-input')).toBeVisible();
    await expect(page.locator('#redit-community-join')).toBeVisible();
  });
});