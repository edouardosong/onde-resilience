import { test, expect } from '@playwright/test';

/**
 * 08 — Interactive social features: votes, bookmarks, reports, messages
 *
 * Mode démo (sans Tauri). Verifies that the UI elements for interactive
 * social features are present and functional (simulated behavior).
 *
 * Note: demo-mode social cards (addSocialCard) do NOT include interactive
 * vote/bookmark/report buttons — those appear only in renderSocialPosts
 * (Tauri mode). Tests here verify the UI surface elements are present.
 */
test.describe('08 — Interactive social: votes, bookmarks, messages', () => {
  test('Tuitter demo post appears with author and content after publish', async ({ page }) => {
    const message = `Interactive test tuit — ${Date.now()}`;

    await page.goto('/');
    await page.locator('.mode-btn[data-mode="tuitter"]').click();

    // Publish a tuit
    await page.locator('#tuitter-compose-input').fill(message);
    await page.locator('#tuitter-compose-send').click();

    // The card should have author "Moi" and the message text
    const card = page.locator('#tuitter-feed-container .card').filter({ hasText: message });
    await expect(card).toHaveCount(1);
    await expect(card).toContainText('Moi');
  });

  test('moderation tab is present in Tuitter nav', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="tuitter"]').click();

    // Moderation tab should be in the nav
    await expect(page.locator('#nav-tuitter .tab[data-page="tuitter-moderation"]')).toBeVisible();

    // Navigate to moderation
    await page.locator('#nav-tuitter .tab[data-page="tuitter-moderation"]').click();
    await expect(page.locator('#tuitter-moderation')).toHaveClass(/active/);
    await expect(page.locator('#tuitter-moderation-list')).toBeVisible();
  });

  test('message composer is present in Tuitter messages page', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="tuitter"]').click();

    // Navigate to messages
    await page.locator('#nav-tuitter .tab[data-page="tuitter-messages"]').click();
    await expect(page.locator('#tuitter-messages')).toHaveClass(/active/);

    // Message composer fields are present
    await expect(page.locator('#msg-recipient-input')).toBeVisible();
    await expect(page.locator('#msg-body-input')).toBeVisible();
  });

  test('bookmarks tab is present in Redit nav', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="redit"]').click();

    await expect(page.locator('#nav-redit .tab[data-page="redit-bookmarks"]')).toBeVisible();

    await page.locator('#nav-redit .tab[data-page="redit-bookmarks"]').click();
    await expect(page.locator('#redit-bookmarks')).toHaveClass(/active/);
    await expect(page.locator('#redit-bookmarks-list')).toContainText('Aucun favori');
  });

  test('moderation tab in Redit nav is reachable', async ({ page }) => {
    await page.goto('/');
    await page.locator('.mode-btn[data-mode="redit"]').click();

    await expect(page.locator('#nav-redit .tab[data-page="redit-moderation"]')).toBeVisible();

    await page.locator('#nav-redit .tab[data-page="redit-moderation"]').click();
    await expect(page.locator('#redit-moderation')).toHaveClass(/active/);
    await expect(page.locator('#redit-moderation-list')).toBeVisible();
  });
});