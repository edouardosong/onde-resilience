import { test, expect } from '@playwright/test';

/**
 * 06 — Tuitter: publication, feed et navigation sociale
 *
 * Mode démo (sans Tauri) — le comportement vérifié est celui du frontend
 * web statique: le bouton d'envoi insère une carte simulée dans le flux.
 * Les parcours Tauri réels (social_publish_post, etc.) ne sont pas
 * accessibles depuis un navigateur Playwright sans binaire Tauri.
 */
test.describe('06 — Tuitter: publication et flux social', () => {
  test('le mode Tuitter est accessible et affiche le flux', async ({ page }) => {
    await page.goto('/');

    // Basculer en mode Tuitter
    await page.locator('.mode-btn[data-mode="tuitter"]').click();
    await expect(page.locator('#tuitter-feed')).toHaveClass(/active/);

    // La barre de composition Tuitter est visible
    await expect(page.locator('#compose-bar-tuitter')).toBeVisible();
    // La nav Tuitter est visible
    await expect(page.locator('#nav-tuitter')).toBeVisible();

    // Le flux Tuitter montre l'état vide initial
    await expect(page.locator('#tuitter-feed-container .empty-state')).toBeVisible();
  });

  test('publication simulée d\'un tuit en mode démo', async ({ page }) => {
    const message = `Tuit de test e2e — ${Date.now()}`;

    await page.goto('/');
    await page.locator('.mode-btn[data-mode="tuitter"]').click();
    await expect(page.locator('#tuitter-feed')).toHaveClass(/active/);

    // Saisie du tuit
    await page.locator('#tuitter-compose-input').fill(message);
    await page.locator('#tuitter-compose-send').click();

    // La carte simulée apparaît dans le flux
    await expect(page.locator('#tuitter-feed-container .card').filter({ hasText: message })).toHaveCount(1);
    // Le champ est vidé
    await expect(page.locator('#tuitter-compose-input')).toHaveValue('');
  });

  test('navigation entre les onglets Tuitter', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="tuitter"]').click();

    // Navigation vers la recherche
    await page.locator('#nav-tuitter .tab[data-page="tuitter-search"]').click();
    await expect(page.locator('#tuitter-search')).toHaveClass(/active/);
    await expect(page.locator('#tuitter-search-input')).toBeVisible();

    // Navigation vers les messages
    await page.locator('#nav-tuitter .tab[data-page="tuitter-messages"]').click();
    await expect(page.locator('#tuitter-messages')).toHaveClass(/active/);

    // Navigation vers le profil
    await page.locator('#nav-tuitter .tab[data-page="tuitter-profile"]').click();
    await expect(page.locator('#tuitter-profile')).toHaveClass(/active/);
    await expect(page.locator('#tuitter-display-name')).toBeVisible();

    // Retour au flux
    await page.locator('#nav-tuitter .tab[data-page="tuitter-feed"]').click();
    await expect(page.locator('#tuitter-feed')).toHaveClass(/active/);
  });

  test('le logo change selon le mode', async ({ page }) => {
    await page.goto('/');

    await expect(page.locator('#logo-text')).toHaveText('⧫ ONDE');

    await page.locator('.mode-btn[data-mode="tuitter"]').click();
    await expect(page.locator('#logo-text')).toContainText('Tuitter');

    await page.locator('.mode-btn[data-mode="redit"]').click();
    await expect(page.locator('#logo-text')).toContainText('Redit');

    await page.locator('.mode-btn[data-mode="onde"]').click();
    await expect(page.locator('#logo-text')).toHaveText('⧫ ONDE');
  });
});