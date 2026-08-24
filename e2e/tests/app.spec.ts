import 'dotenv/config';
import { test as base, expect } from '@playwright/test';
import { PlaywrightAiFixture } from '@midscene/web/playwright';

// Midscene.js v1.11 — fixture IA pour @playwright/test :
// chaque test reçoit aiTap / aiInput / aiAssert (screenshot → LLM local → action).
const test = base.extend(PlaywrightAiFixture());

const APP_URL = 'file:///home/linux/Documents/CleanOnde/android/app/src/main/assets/index.html';

// Chargement stable : attendre la fin du load + un settle avant les appels IA
// (évite le race 'Execution context was destroyed' lors de l'injection Midscene).
async function loadApp(page: import('playwright').Page) {
  await page.goto(APP_URL);
  await page.waitForLoadState('load');
  await page.waitForTimeout(400);
}

test.describe('ONDE — UI (Midscene.js + Playwright, LLM local)', () => {
  test('T1 état initial : header, badge mesh, feed avec PoW', async ({ page, aiAssert }) => {
    await loadApp(page);
    await aiAssert("L'écran affiche le logo ONDE en haut, un badge indiquant que le mesh est actif, et au moins une carte de message contenant la mention 'PoW vérifié'");
  });

  test('T2 onglet Entraide filtre le feed (seules les cartes ENTRAIDE restent visibles)', async ({ page, aiTap, aiAssert }) => {
    await loadApp(page);
    await aiTap("le bouton 'Entraide' de la barre de filtres au-dessus du flux");
    await aiAssert("Dans le flux, toutes les cartes visibles portent le badge ENTRAIDE ; aucune carte ALERTE n'est visible");
  });

  test('T3+T4 composer un message depuis l\'onglet Entraide (bouton puis touche Entrée)', async ({ page, aiTap, aiInput, aiAssert }) => {
    await loadApp(page);
    await aiTap("le bouton 'Entraide' de la barre de filtres au-dessus du flux");
    await aiInput("TEST MIDSCENE — message d\'entraide publié depuis l\'onglet Entraide", "le champ de saisie 'Alerte ou message d\'entraide' en bas de l'écran");
    await aiTap("le bouton vert d'envoi à droite du champ de saisie");
    await aiAssert("La première carte du flux est signée 'Moi', porte le badge ENTRAIDE et contient le texte 'TEST MIDSCENE'");

    // publication via la touche Entrée (le champ est déjà focalisé après l'envoi)
    await page.keyboard.type('TEST MIDSCENE — publié avec la touche Entrée');
    await page.keyboard.press('Enter');
    await aiAssert("La première carte du flux est signée 'Moi', porte le badge ENTRAIDE et contient 'publié avec la touche Entrée'");
  });

  test('T5 assistant IA répond à une question', async ({ page, aiTap, aiInput, aiAssert }) => {
    await loadApp(page);
    await aiTap("l'onglet 'IA' de la barre de navigation en bas");
    await aiInput("Comment purifier de l'eau en urgence ?", "le champ 'Posez votre question...' du chat IA");
    await aiTap("le bouton d'envoi (flèche) à droite du champ du chat IA");
    await page.waitForTimeout(2500); // réponse simulée ~800 ms + marge
    await aiAssert("Le fil de discussion contient ma question ET une bulle de réponse de l'assistant mentionnant PocketPal ou Qwen");
  });

  test('T6 wallet : bouton Recevoir affiche le code du nœud', async ({ page, aiTap, aiAssert }) => {
    await loadApp(page);
    await aiTap("l'onglet 'Wallet' de la barre de navigation en bas");
    await aiAssert("Le solde en crédits est affiché et l'historique de transactions est visible");
    await aiTap("le bouton 'Recevoir'");
    await aiAssert("Un panneau s'affiche avec un code de réception du nœud au format u09tunq-ONDE");
  });

  test('T7 encyclopédie : recherche met à jour les résultats', async ({ page, aiTap, aiInput, aiAssert }) => {
    await loadApp(page);
    await aiTap("l'onglet 'Wiki' de la barre de navigation en bas");
    await aiInput("purification de l'eau", "le champ de recherche Wikipédia hors-ligne");
    await aiTap("le bouton de recherche (icône loupe) à droite du champ");
    await aiAssert("Les résultats affichés correspondent à la requête 'purification de l\\'eau'");
  });

  test('T8 P2P : zone QR d\'appairage et zone de dépôt fichier visibles', async ({ page, aiTap, aiAssert }) => {
    await loadApp(page);
    await aiTap("l'onglet 'P2P' de la barre de navigation en bas");
    await aiAssert("La page affiche une zone QR d'appairage et une zone de dépôt de fichier indiquant qu'on peut taper pour sélectionner un fichier");
  });

  test('T9 voix : bouton lecture du mémo vocal est réactif', async ({ page, aiTap, aiAssert }) => {
    await loadApp(page);
    // la carte VOIX (Claire D.) est visible dans l'onglet Alertes par défaut
    await aiTap("le bouton lecture triangulaire du lecteur vocal sur la carte 'Mémo vocal'");
    await aiAssert("Le bouton du lecteur vocal affiche maintenant un symbole de pause, indiquant que la lecture a démarré");
  });

  test('T10 radar : geohash et nœuds rendus', async ({ page, aiTap, aiAssert }) => {
    await loadApp(page);
    await aiTap("l'onglet 'Radar' de la barre de navigation en bas");
    await aiAssert("La page affiche une valeur Geohash, des statistiques (nœuds proches, ponts desktop) et un radar circulaire avec des points de nœuds");
  });
});
