# ONDE — Rapport de tests E2E sur appareils réels + corrections (T32-B)

**Date** : 2026-08-23 · **Repo** : /home/linux/Documents/CleanOnde · **Branche** : fix/e2e-bugs-t32b
**Appareils** : A = Xiaomi 11T (lisa, Android 13, 192.168.1.98) · B = Pad 5 (nabu, Android 15, 192.168.1.87)
**Méthode** : APK debug installé sur les deux appareils ; pilotage UI via CDP
(`adb forward tcp:922x localabstract:webview_devtools_remote_<pid>` + Runtime.evaluate),
suite de 11 scénarios exécutée **sur chaque appareil**, logcat crash buffer vide des deux côtés.

## Prérequis testabilité (modifié)
- `MainActivity.java` : `webView.setWebContentsDebuggingEnabled(true)` — DevTools Protocol sur la
  WebView, accessible uniquement en local via adb forward. À conditionner au build debug à terme.

## Bugs trouvés et corrigés (vérifiés sur A ET B après chaque fix)

| # | Scénario | Bug constaté (preuve E2E avant fix) | Correction | Statut après fix |
|---|----------|--------------------------------------|------------|------------------|
| 1 | T2 onglets Alertes/Entraide | Le clic ne changeait que le style des boutons : feed non filtré (`cards 6→6; types avant=['ALERTE','ENTRAIDE',...] après=identique`) sur A et B | `activeFeedTab` + `applyFeedFilter()` : les cartes portent `data-type` (alert/aid/voice) ; onglet Alertes → alert+voice, onglet Entraide → aid uniquement ; re-filtrage à chaque insertion | **PASS** `cards 3→1; après=['ENTRAIDE']` sur A et B |
| 2 | T3 type de la carte publiée | Compose publiait toujours un badge ALERTE même depuis l'onglet Entraide (`première={'name':'Moi','badge':'ALERTE'}`) | `publishMessage()` : type/badge/avatar suivent `activeFeedTab` (ENTRAIDE bleu dans le contexte Entraide) | **PASS** `première={'name':'Moi','badge':'ENTRAIDE'}` sur A et B |
| 3 | T4 touche Entrée | `cards 8→8` : la touche Entrée ne publiait rien (aucun handler keydown) | Listener `keydown` Enter → `publishMessage()` (même chemin que le bouton ⧫) | **PASS** `cards 4→5` sur A et B |
| 4 | T6 wallet « Recevoir » | Bouton mort : cliqué=True, aucun effet (`tx 3→3`) | `id="btn-receive"` + panneau `#receive-panel` (code de réception du nœud) en toggle au clic | **PASS** `panneau après clic 1='block' code='u09tunq-ONDE' après clic 2='none'` sur A et B |
| 5 | T8 P2P « taper pour sélectionner » | Zone drop inerte : `input[type=file] = 0`, aucun handler click/change | `<input type="file" id="p2p-file-input">` caché + click sur `#file-drop` → picker ; feedback nom+taille du fichier sélectionné | **PASS** `label='✓ plan-eau.pdf (2 Ko) — prêt à partager via le mesh'` sur A et B |
| 6 | T9 bouton lecture voix | Bouton ▶ inerte (`avant='▶' après='▶'`) | Toggle ▶/⏸ + classe `.playing` (état visuel du lecteur vocal) | **PASS** `avant='▶' après='⏸'` sur A et B |

## Corrections mineures (relecture code, appliquées dans le même lot)
- **Nommage IA incohérent** : badge « Qwen 1.8B (Q4_K_M) » vs méta des réponses « PocketPal ».
  Alignement sur le core Rust (`PocketPalEngine`, `ModelSize::Qwen1_8B`) : partout
  « Moteur local : PocketPal · Qwen 1.8B ». Vérifié T5 sur A et B.
- **Chat IA non scrollable** : `.ai-chat .messages` sans `overflow-y:auto` → ajout + `min-height:0`.
- **Recherche wiki vide silencieuse** : hint « Saisissez un terme de recherche… » affiché 2,5 s.
- **Cohérence XSS** : les insertions du feed simulé (`post.name`, `post.content`) passent désormais
  par `esc()` comme le chemin compose (politique déclarée en tête de script).

## Résultats finaux (suite complète, après tous les fixes)
- Appareil A (11T) : **11/11 PASS** — T1..T11, console JS sans erreur.
- Appareil B (Pad 5) : **11/11 PASS** — T1..T11, console JS sans erreur.
- Logcat crash buffer : vide sur les deux appareils ; affichage +480 ms (B).

## Non bloquants / à suivre (pas des bugs de cette itération)
- Badge « Mesh faible » qui clignote aléatoirement (5 % toutes les 5 s) : simulation assumée du
  statut mesh, à brancher sur l'état réel du transport TCP (T32-B/C).
- QR P2P = glyphe placeholder (pas un vrai QR encodant le code d'appairage) — stub de démo.
- `setWebContentsDebuggingEnabled(true)` : à conditionner au build debug avant release.

## Repro
```bash
# build + install sur les deux appareils
cd android && ANDROID_HOME=/opt/android-sdk ./gradlew assembleDebug --no-daemon
adb -s <SERIAL_A> install -r app/build/outputs/apk/debug/app-debug.apk   # idem B
adb -s <SERIAL_X> shell am start -n com.onde.resilience/.MainActivity
# CDP : forward + ws URL via /json/list, puis driver Runtime.evaluate (11 scénarios)
```
