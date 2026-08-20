# ONDE — E2E UI (Playwright) — L2-06

Baseline E2E de l'UI ONDE, portée des 5 plans legacy `testsprite/*.plan.json`
(TestSprite payant remplacé par Playwright MIT — décision 2026-08-20,
100% gratuit / open source, aucune clé API).

## Exécuter

```bash
cd ui/web
npm install                 # @playwright/test + playwright (devDependencies)
npx playwright test         # 3 specs × … : 5 fichiers, chromium headless
npx playwright show-report  # rapport HTML (playwright-report/)
```

La config (`playwright.config.ts`) démarre elle-même le dev server Vite
(`npm run dev -- --port 5173 --strictPort --host 127.0.0.1`, root `ui/src/`)
via `webServer` — pas besoin de Tauri pour tester le web.

- Navigateur : chromium **headless** (headless shell Playwright, révision 1234)
- `retries: 1`, `workers: 1`, `timeout: 30 s`
- Évidence : **screenshots (par action) + traces** dans `e2e-results/` ;
  rapport HTML dans `playwright-report/`
- Les deux dossiers sont gitignorés (artéfacts, pas du code).

## Mapping plans legacy → specs

| Plan legacy | Spec | Statut |
|---|---|---|
| 01-node-startup | `e2e/01-node-startup.spec.ts` | ✅ PASS — identité + état de santé (header + stats Radar). GAP : panneau logs/santé + réputation globale (absents de l'UI) |
| 02-publish-alert | `e2e/02-publish-alert.spec.ts` | ✅ PASS — saisie alerte → carte en tête du flux, type ALERTE, badge PoW démo. GAP : signature réelle + ID d'événement (mode Tauri uniquement), champ zone géographique (absent) |
| 03-mutual-aid | `e2e/03-mutual-aid.spec.ts` | ✅ PASS — mode Entraide actif + carte publiée en tête du flux. Écart documenté : le badge de type est hardcodé « ALERTE » en mode démo (comportement de l'app) |
| 04-feed | `e2e/04-feed.spec.ts` | ✅ PASS — cartes typées (ALERTE/ENTRAIDE/VOIX), auteurs, horodatage flou (« Il y a… »), stabilité des basculements Alertes/Entraide. Écart documenté : les boutons règlent le mode de publication, le flux affiché n'est PAS filtré par type |
| 05-reputation-wot | `e2e/05-reputation-wot.spec.ts` | ✅ PASS (test baseline état du mesh) + ⛔ SKIP documenté — l'écran Réputation/WoT n'existe pas dans l'UI (composant manquant ; `get_reputation` Tauri non exposé au web) |

## Règles d'honnêteté appliquées

- Aucune assertion sur un comportement que l'UI n'a pas (badge ENTRIADE en démo,
  filtrage du flux, écran WoT) : documenté en `comment` dans la spec + ici.
- Assertions relatives (comptage avant/après) pour résister aux timers de démo
  de l'app (carte auto-toutes-les-30 s, badge mesh aléatoire toutes les 5 s).
- Le scénario WoT complet reste déclaré (test `skip` avec raison) pour ne pas
  oublier la couverture attendue.
