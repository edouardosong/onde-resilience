# AGENT : Développeur Frontend (Tauri + React)

## Mission
Implémenter et maintenir l'UI Tauri/React (navigation, affichage AMOLED black, dashboard) et le pont Rust↔JS.

## Stack proposée (L'ACTEUR CHOISIT ses outils dans cette liste)

### Framework
- [ ] React 18/19
- [ ] TypeScript strict
- [ ] Vite
- [ ] Tauri 2.x

### Styling
- [ ] Tailwind CSS
- [ ] CSS Modules

### Test
- [ ] Vitest
- [ ] Testing Library
- [ ] Playwright (E2E)

### Qualité
- [ ] ESLint
- [ ] Prettier
- [ ] zod (validation)
- [ ] knip (dead code)

## Choix de stack (rempli par l'acteur)
>(à déposer par l'agent dans ce fichier sous « ## CHOIX EFFECTUÉ », avec justification.)

## CHOIX EFFECTUÉ (déposé par dev_frontend)

### Stack retenue
- [x] React 19 + TypeScript strict (indispensable)
- [x] Vite 7/8 + Tauri 2.x (indispensable — déjà amorcé)
- [x] Tailwind CSS (AMOLED theme tokens)
- [x] Zustand (état) — léger, adapté au pont temps réel
- [x] React Router v7 (navigation par pages Feed/Radar/IA/Wallet/Wiki/P2P)
- [x] zod (validation des payloads du pont Rust↔JS)
- [x] Vitest + Testing Library (tests unitaires + composants)
- [x] Playwright (E2E sur WebView / build de prod)
- [x] ESLint + Prettier + knip (qualité, format, dead code)

### Justification (1 ligne par item)
- React 19 + TS strict : typage fort du pont rust et de l'arbre de composants ; React 19 déjà dans package-lock, strict mode = garde-fou sur des payloads interprétés hors-ligne.
- Vite + Tauri 2 : déjà amorcé (`ui/web/package.json` : vite 8, @tauri-apps/api 2, plugin-shell), rapide, HMR, frontendDist/devUrl déjà configurés.
- Tailwind CSS : designs tokens de l'AMOLED black (--black/--accent #00FF88/--surface) transposables en `@theme` Tailwind v4 → cohérence cross-page sans CSS pur massif.
- Zustand : store unique + `useSyncExternalStore`-like pour les flux `get_feed_events`/`node_status` polling ; évite le boilerplate Redux, scale suffisant prototype.
- React Router v7 : découpe 6 pages (feed, radar, ai, wallet, wiki, p2p) en routes, état d'onglet partagé entre `.nav-tabs` et URL.
- zod : schémas TS des réponses Tauri (`node_start → pubkey`, `get_feed_events → FeedEventView[]`) — valide ce que Rust renvoie avant de l'afficher (XSS/robustesse).
- Vitest + Testing Library : unit/composant rapide (>= intégré Vite), test des parsers zod et des composants (renderFeed, naviguer).
- Playwright : E2E de la vraie app buildée dans Tauri webview / preview, vérifie navigation + pont hors runtime.
- ESLint + Prettier + knip : lint strict, format uniforme, suppression du code mort (ex. ancien index.html inline) lors de la migration SPA.

### Architecture prévue
- `ui/web/src/` SPA : `main.tsx`, `App.tsx` (routes + layout AMOLED), `pages/*`, `components/*`, `lib/tauri.ts` (wrapper pont typé), `lib/schemas.ts` (zod), `stores/mesh.ts` (Zustand).
- `ui/src/index.html` actuel → migre en template Vite standard pointé par `vite.config.ts` (root `ui/web`, outDir `ui/dist`), `tauri.conf.json` frontendDist ajusté.
- Pont typé : une couche `invoke<C extends keyof Commands>(cmd, args)` + schémas zod → sérialise/dé-sérialise les 8 commandes du `commands.rs`.

### Indispensables (si devoir prioriser)
React+TS strict, Vite+Tauri 2, Zustand, zod, Vitest+Testing Library, ESLint+Prettier+knip.
Les options : Tailwind (remplaçable par CSS Modules), React Router (remplaçable par état d'onglet + Context), Playwright (peut attendre prototype).
## Rôle dans la boucle
- maker / checker / arbitre / analyse — (voir procédures) 
