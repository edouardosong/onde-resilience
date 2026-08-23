# AGENT : Ingénieur CI/CD & SRE

## Mission
Construire les pipelines CI/CD, la matrice de build/test/release mobile, la qualité et le monitoring (GitHub Actions).

## Stack proposée (L'ACTEUR CHOISIT ses outils dans cette liste)

### CI
- [x] GitHub Actions
- [x] matrix builds
- [x] fastlane (mobile release)

### Conteneurs
- [x] Docker
- [x] docker compose (déjà présent dans le repo)

### Qualité/métrique
- [x] CodeRabbit / review bot
- [x] coverage (codecov/cargocov)
- [ ] SonarQube (option — non retenu)

### Ops
- [x] gh CLI
- [x] CI debug shell
- [x] scheduled workflows (quality gates)

## CHOIX EFFECTUÉ (2026, agent CI/CD & SRE)

### Indispensables (requis pour porter ce monorepo en prod)
1. **GitHub Actions** — le repo est hébergé sur GitHub ; il possède déjà `.github/workflows/ci.yml`
   (rust build+test et ui web build), donc rester sur la plateforme native évite un transport/self-host.
   C'est la dalle unique pour build/test/release/planification.
2. **Matrix builds** — le monorepo est cross-platform : `core/` (Rust desktop + cibles Android NDK
   aarch64/armv7/x86_64), `android/` (APK), `ui/` (Tauri+React). Une matrice permet de tester
   plusieurs cibles/toolchains en parallèle sans dupliquer le YAML.
3. **Coverage (cargocov / codecov)** — core est le cœur cryptographique (Ed25519, X25519+HKDF+zeroize,
   ChaCha20-Poly1305, vérification de signature APK). Sans gate de coverage, un refactor cryptographique
   peut rester sans test critique. codecov centralise la lecture (README en attend déjà un badge).
4. **CodeRabbit / review bot** — gate de revue automatisée de chaque PR, complément du checker humain
   de la boucle. Non bloquant de base, mais il nourrit la gate qualité (TRIGGERS.md §3 « Revue de PR »).
5. **fastlane (mobile)** — sortir un APK signé + versionné proprement (versionCode/name déjà dans
   `android/app/build.gradle`) sans scripts fragile ad hoc (`android/build.sh`). Préparera upload
   Google Play/SideStore plus tard.

### Utiles / ponctuels (activés, mais non bloquants)
6. **Docker + docker compose** — déjà fourni (`Dockerfile.dev`, `docker-compose.yml`). Servira pour un
   job **simulation** reproductible (démo `simulation/mesh_sim.py` → artifact JSON) et pour reproduire
   l'environnement de dev exact dans CI.
7. **gh CLI** — action de debug shell (gist de logs), création de releases, agrégation d'issues/PR dans
   le triage quotidien (TRIGGERS.md §1).
8. **Scheduled workflows (quality gates)** — gate de « qualité » hebdomadaire : `cargo` + `npm audit` +
   dépendances obsolètes, badge dégradé, rapport dans STATE.md.

### Non retenu
- **SonarQube** — surcoût infra (instance à héberger/self-host) sans gap vs clippy + codecov + CodeRabbit
  pour un prototype non audité. À réintroduire après le premier audit formel si souhaité.

## Pipeline CI proposé — 3 jobs (rust / android / ui) + gates de qualité

Le `.github/workflows/ci.yml` actuel ne contient que 2 jobs partiels (rust build+test, ui web build)
et n'exécute ni `clippy`, ni `fmt`, ni coverage, ni job Android, ni release. La cible :

### ▸ Job 1 — `rust` (core/, le cœur)
- `dtolnay/rust-toolchain@stable` + composants `rustfmt` et `clippy`.
- Cache cargo (`core/target`, registry, git) par hash de `core/Cargo.lock`.
- **Gate qualité rust** : `cargo fmt --check` → `cargo clippy --workspace --all-targets -- -D warnings`
  → `cargo test --workspace` (unit + doc) → `cargo test --test integration_e2e` (tests e2e dans `core/tests/`).
- **Coverage** : `cargo llvm-cov` (ou `tarpaulin`) + upload codecov (gate si < seuil, ex. 70%).
- Release conditionnelle (push tag `v*`) : `cargo build --release --workspace` puis upload des binaires.

### ▸ Job 2 — `android` (app/ + core NDK)
- `actions/setup-java@temurin-17` + `android-actions/setup-android@` (SDK 34, NDK `26.2.11394342`
  — à aligner sur `.cargo/config.toml` et `Dockerfile.dev`).
- Générer le `gradlew` wrapper (absent du repo : seul `gradle-wrapper.properties` existe) et `./gradlew --version`.
- **Install des targets Rust Android** : `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`
  → `cargo build --release --target aarch64-linux-android` (cœur NDK).
- `./gradlew assembleDebug` + `./gradlew lint` (gate qualité Android : Zero critical).
- `./gradlew assembleRelease` + signature test → artefact APK uploadé (vers release sur tag).
- (option) **fastlane** `lane android build` + `lane android beta` (upload APK signé).

### ▸ Job 3 — `ui` (Tauri + React)
- `actions/setup-node@20` + `npm ci` (cache `ui/web/node_modules`).
- `npm run build` (vite) — le frontend React.
- **Gate qualité ui** : `npm audit --audit-level=high` (dépendances) ; `eslint`/`tsc` si ajoutés.
- **Tauri rust** (dépendances système `libwebkit2gtk-4.0`, `libgtk-3`, `libayatana-appindicator3`,
  requises par Linux) : `cargo clippy` + `cargo test` dans `ui/src-tauri` — même drive que le job rust.

### Gates de qualité transverses
- **Blocage** (fail = pas de merge) : `fmt`, `clippy -D warnings`, tests (rust+android+ui), lint Android,
  `npm audit high`, coverage sous seuil.
- **Non bloquant (reporté)** : CodeRabbit (revue), scheduled weekly « qualité » (cargo audit + npm audit),
  debug shell (gist) sur échec.
- `permissions: contents: read` conservé (least-privilege, déjà en place).
- Déclencheurs : `push`/`PR` sur `main` **+ tags `v*`** (release) **+ scheduled `0 9 * * 1`** (quality gate).

## Rôle dans la boucle
- maker / checker / arbitre / analyse — (voir procédures)
