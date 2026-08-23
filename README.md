# ⧫ ONDE — Réseau de Résilience Citoyen

> **Infrastructure de communication mesh hors-ligne : alertes, social, voix, IA locale — sans internet, sans cloud, sans serveur central.**

[![CI](https://github.com/edouardosong/onde-resilience/actions/workflows/ci.yml/badge.svg)](https://github.com/edouardosong/onde-resilience/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Toolchain](https://img.shields.io/badge/rust%20toolchain-1.96.0%20épinglée-orange)](rust-toolchain.toml)
[![Tests core](https://img.shields.io/badge/tests%20core-329%20passing-brightgreen)](#-tests--qualité)
[![Simulation](https://img.shields.io/badge/simulation-23%20passing-brightgreen)](#-tests--qualité)
[![Coverage](https://img.shields.io/badge/coverage%20core-89,8%25%20lignes-informational)](reports/t22-coverage/proposition-gate.md)
[![AST gate](https://img.shields.io/badge/AST%20gate-ast--grep%20rules-success)](.ast-grep/sgconfig.yml)

---

## Statut du projet

**Prototype avancé, durci de manière continue** — le cœur réseau, cryptographique et de stockage est implémenté, testé adversarialement (mutation testing, fuzzing, sondes adversariales) et validé à l'échelle en simulation. **Ce n'est pas encore un produit de production** : l'audit de sécurité professionnel (Phase 3.1) reste à faire, et les preuves ZK sont un mock assumé.

| Verdict | Périmètre |
|---|---|
| ✅ **Réel et durci** | Mesh DTN, crypto Ed25519/X25519/ChaCha20-Poly1305, PoW adaptatif, réputation Web-of-Trust propagée, padding de trafic, mises à jour signées par le mesh, persistance SQLite, auto-réparation de partition, observabilité |
| 🔶 **Réel derrière feature optionnelle** | LLM local (llama.cpp/GGUF, feature `llama-cpp`), STT (whisper.cpp, feature `whisper-cpp`) — mocks par défaut pour les builds légers |
| 🟡 **Récent, validé en cours** | Budget mémoire 100k messages (stress release exécuté, pic RSS borné — revue finale en cours) |
| ⚠️ **Mock assumé** | Preuves ZK (`ZkProof::verify` non implémenté — ROADMAP 3.9 : SNARK réel ou retrait du claim) |

---

## Vue d'ensemble

ONDE permet à deux nœuds (mobiles ou desktops) d'**échanger des alertes signées, chiffrées, propagées et persistées en marchant côte à côte** — sans aucune infrastructure. Le transport Wi-Fi Aware/BLE se câble sur du matériel physique ; tout le reste (protocole, DTN, stockage, IA locale, mises à jour) est fonctionnel aujourd'hui.

### Fonctionnalités

| Module | Description | Statut |
|---|---|---|
| 🔄 **Mesh & DTN** | Store-and-forward priorisé, rencontres opportunistes O(1) (bucketing multi-tiers), TTL, déduplication | ✅ Réel |
| 🔐 **Crypto** | Ed25519 (identités), X25519 ECDH + HKDF-SHA256 + ChaCha20-Poly1305 (E2E), rotation d'identité avec période de grâce | ✅ Réel |
| 🛡️ **Anti-abus** | PoW adaptatif + Web of Trust : endossements propagés dans le mesh, fenêtre glissante par auteur, attaque de spam contenue (testée) | ✅ Réel |
| 📝 **Social hors-ligne** | Deux plateformes sur le même graphe : **Tuitter** (micro-blog) et **Redit** (communautés) — événements signés, cache SQLite matérialisé | ✅ Réel |
| 🧠 **IA locale** | Inférence GGUF réelle via llama.cpp (Qwen 0.5B–7B selon RAM) derrière feature `llama-cpp`; oracle RPC desktop | 🔶 Feature flag |
| 🎙️ **Voix** | STT Whisper réel via whisper.cpp derrière feature `whisper-cpp` ; mémos vocaux Opus via DTN | 🔶 Feature flag |
| 📚 **Encyclopédie** | Lecteur ZIM réel : compression none/lz4/zstd, entrées/recherches titres, durci adversarialement (fuzz + mutants) | ✅ Réel |
| 🗺️ **Cartes offline** | Parser MBTiles réel : métadonnées, schéma 1.3, tuiles z/x/y TMS↔XYZ | ✅ Réel |
| 📦 **Mises à jour OTA** | Canal signé par le mesh : annonce + manifeste (racine épinglée) + chunks + `verify_apk_signature()` de bout en bout | ✅ Réel |
| 🕶️ **Confidentialité** | Padding de trafic opérationnel sur le fil (seaux 256 B / 1 Kio / 4 Kio / 16 Kio) | ✅ Réel |
| ♻️ **Auto-réparation** | Détection de partition déterministe, re-sync au retour, heal borné (zéro perte/duplication testés) | ✅ Réel |
| 📊 **Observabilité** | Métriques atomiques (ingestion/gossip/pairs/storage), snapshot JSON au démarrage, endpoint santé `GET /health` **opt-in** localhost | ✅ Réel |
| 💾 **Budget mémoire** | Stress 100k messages par le chemin réel d'ingestion : pic RSS mesuré et borné, compression Deflate validée | 🟡 Revue finale |
| 💰 **Finance ZK** | Transactions asynchrones type Mina | ⚠️ Mock assumé |

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                     Couches présentation                      │
│  UI web standalone (AMOLED Black) · Tauri desktop · Android   │
├──────────────────────────────────────────────────────────────┤
│                    Bridge Tauri (Rust)                        │
│        commandes social_* · node_start · publication          │
├──────────────────────────────────────────────────────────────┤
│                      onde_core (Rust)                         │
│  ┌────────────┬────────────┬─────────────┬─────────────────┐ │
│  │ network    │ protocol   │ crypto      │ storage         │ │
│  │ gossip,    │ MeshEvent, │ Ed25519,    │ hiérarchique +  │ │
│  │ pairs,     │ kinds 1-21,│ X25519,     │ SQLite WAL,     │ │
│  │ padding    │ PoW, wire  │ ChaCha20,   │ sharding Geohash│ │
│  └────────────┴────────────┴─────────────┴─────────────────┘ │
│  ┌────────────┬────────────┬─────────────┬─────────────────┐ │
│  │ reputation │ social     │ update      │ health/metrics  │ │
│  │ WoT mesh,  │ Tuitter/   │ APK signées │ métriques atom. │ │
│  │ anti-spam  │ Redit      │ chunks+root │ endpoint opt-in │ │
│  └────────────┴────────────┴─────────────┴─────────────────┘ │
├──────────────────────────────────────────────────────────────┤
│                       Crates spécialisées                     │
│  dtn-router · zim-parser · mbtiles-parser · llm-inference     │
│  llama-bind (feature llama-cpp) · whisper-stt (whisper-cpp)   │
├──────────────────────────────────────────────────────────────┤
│              Transport (branchable) : Wi-Fi Aware,            │
│              BLE, LoRa/Meshtastic, Ethernet bridge            │
└──────────────────────────────────────────────────────────────┘
```

---

## Démarrage rapide

Prérequis : [Rust](https://rustup.rs) (la toolchain **1.96.0** est épinglée via `rust-toolchain.toml`, installée automatiquement), optionnellement [uv](https://docs.astral.sh/uv/) pour la simulation.

### Construire et tester le moteur

```bash
git clone https://github.com/edouardosong/onde-resilience.git
cd onde-resilience/core

cargo build --release          # binaire : target/release/onde_node
cargo test --workspace --locked   # 18 suites, 329 tests
```

### Lancer un nœud

```bash
./target/release/onde_node \
    --type mobile \
    --name "Node-A" \
    --geohash u09tunq \
    --db ./node-a.sqlite
#     --health-port 8080     # optionnel : GET http://127.0.0.1:8080/health (JSON)
#     --battery-saver        # optionnel : travail de fond throttlé
```

Options réelles (`onde_node --help`) : `--type <mobile|desktop>` · `--name` · `--db <chemin>` · `--geohash <7 car>` · `--health-port <port>` (**opt-in**, écoute uniquement sur 127.0.0.1) · `--battery-saver`.

Deux nœuds lancés sur la même machine/deux machines voisines échangent leurs événements par gossip : chaque alerte publiée est signée, padée, propagée, persistée, et survit au redémarrage du nœud (restauration SQLite).

### Simulation réseau (SimPy)

```bash
uv sync --project simulation                          # env reproductible (uv.lock versionné)
uv run --project simulation pytest simulation/ -q     # 23 passed
uv run --project simulation python simulation/mesh_sim.py   # run par défaut
```

Validé à l'échelle ROADMAP (Phase 3.3) : **11 000 nœuds**, seed=42 déterministe (rapports byte-identiques), ~41 s wall-time, 46,5 Mo de RAM pic. Loi mesurée : `wall ≈ 29,7 s + 1,15 ms/nœud` — le coût dominant est le PoW adaptatif, pas la taille du mesh. Détails : [`reports/t25-sim11k/rapport.md`](reports/t25-sim11k/rapport.md).

### Interface utilisateur

```bash
# Mode développement (Vite + e2e Playwright)
cd ui/web
npm ci
npm run dev                # UI de dev
npm run test:e2e           # suite Playwright (webServer auto)

# Application desktop (Tauri)
cd ui/src-tauri && cargo tauri build

# Android (projet Gradle dédié)
cd android && ./gradlew assembleDebug
```

### Docker (environnement de dev complet)

```bash
docker compose build
docker compose run dev bash       # Rust + Python + Android SDK préinstallés
```

---

## Tests & qualité

Le projet est développé sous une **boucle d'agents auditable** (détecter → planifier → implémenter → vérifier indépendamment → merger → mémoriser) dont la colonne vertébrale est versionnée dans [`.agentloop/`](.agentloop/STATE.md) et les preuves dans [`reports/`](reports/loop-health.md). Chaque merge passe les gates suivantes :

| Gate | Commande / outil | Critère |
|---|---|---|
| Tests | `cargo test --workspace --locked` | **329 tests, 18 suites, 0 échec** |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` | 0 warning |
| Format | `cargo fmt --all -- --check` | clean |
| Secrets | `gitleaks detect` (+ historique complet) | bloquant, 0 leak |
| AST | `ast-grep scan -c .ast-grep/sgconfig.yml` | aucun finding nouveau vs [`reports/ast-grep-baseline.txt`](reports/ast-grep-baseline.txt) (anti-`unwrap`/`unsafe` hors tests dans modules critiques crypto/network/storage/protocol/reputation) |
| Preuve négative | `cargo-mutants` ciblé sur le diff | cluster pertinent caught ; passes archivées ([t21](reports/t21-mutants/), [t23](reports/t23-mutants/)) |
| Coverage | cargo-llvm-cov, baseline 2026-08-23 | core = **89,80 % lignes** ; gate diff ≥ 80 % ±2 pts décidée |
| E2E UI | `npx playwright test` (ui/web) | 5 specs + 1 skip documenté |
| Simulation | `uv run --project simulation pytest -q` | 23 passed, déterminisme seed=42 |
| Fuzzing | `cargo fuzz list` (5 cibles : protocole, crypto, parsing) | 0 crash exploitable (~130M cas, coverage stable) |

Règle transversale : **un maker ne vérifie jamais son propre code** — chaque tâche passe par une instance checker indépendante qui rejoue elle-même les gates et rapporte ses propres exit codes.

---

## Sécurité

| Couche | Mécanisme |
|--------|-----------|
| Identité | Ed25519 par nœud ; rotation X25519 annoncée dans le mesh avec période de grâce (forward secrecy) |
| Chiffrement | X25519 ECDH + HKDF-SHA256 + ChaCha20-Poly1305, bout en bout |
| Anti-spam | PoW CPU adaptatif (difficulté selon la réputation) + Web of Trust propagée |
| Intégrité des mises à jour | Annonce + manifeste canonique signés, racine de distribution épinglée, SHA-256 du fichier entier avant installation |
| Confidentialité | Padding de trafic en seaux sur le fil — la taille observée ne révèle jamais la taille réelle |
| Résilience stockage | Persistance SQLite WAL + magasin hiérarchique compressé (Deflate) + restauration complète au démarrage |
| Supply chain | Lockfiles épinglés (`--locked`, `npm ci`), scans gitleaks bloquants, toolchain Rust épinglée |
| Endpoint santé | Opt-in, bind 127.0.0.1 uniquement, plafond de connexions + budget anti-slowloris testés adversarialement |

Signalement : voir [`SECURITY_FIX_SUMMARY.md`](SECURITY_FIX_SUMMARY.md) pour l'historique des corrections, et la section Roadmap pour l'audit professionnel planifié.

---

## Structure du projet

```
onde-resilience/
├── core/                     # Moteur Rust (workspace, toolchain 1.96.0 épinglée)
│   ├── src/                  #   onde_core : network, protocol, crypto, storage,
│   │                         #   node, ai, reputation, social, update, health, metrics
│   ├── src/bin/node.rs       #   onde_node — binaire daemon
│   ├── crates/               #   dtn-router · zim-parser · mbtiles-parser ·
│   │                         #   llm-inference · llama-bind · whisper-stt
│   ├── tests/                #   intégration + e2e multi-nœuds
│   └── fuzz/ … ../fuzz/      #   cibles cargo-fuzz
├── ui/
│   ├── src/index.html        # UI standalone AMOLED Black (ouverte dans un navigateur)
│   ├── web/                  # Vite + Playwright (dev + e2e)
│   └── src-tauri/            # App desktop Tauri
├── android/                  # Projet Gradle (wrapper commité)
├── simulation/               # Simulateur SimPy (mesh_sim.py) + suite pytest
├── fuzz/                     # Harness cargo-fuzz (5 cibles)
├── docs/adr/                 # ADRs (rencontres, format wire social)
├── reports/                  # Preuves de la boucle : gates, mutants, simulations, santé
├── .agentloop/               # Colonne vertébrale de la boucle d'agents (STATE.md versionné)
├── ROADMAP.md                # Feuille de route v3.0.0 détaillée
├── rust-toolchain.toml       # Toolchain épinglée (CI = local)
└── docker-compose.yml        # Services dev (onde-dev) + simulation (onde-sim)
```

---

## Roadmap

La feuille de route complète vit dans [`ROADMAP.md`](ROADMAP.md). État au 2026-08-23 :

- **Phase 1 (v1.0.0) — réseau fonctionnel, démo bout en bout** : ✅ atteinte (alerte critique signée → propagée → persistée → restaurée → re-propagée sans internet, prouvée par e2e `test_e2e_critical_alert_full_lifecycle`)
- **Série L2 (durcissement)** : ✅ 14/14
- **Phase 2 (v2.0.0) — intelligence locale** : 2.1 LLM ✅ · 2.2 STT ✅ · 2.3 ZIM ✅ · 2.4 MBTiles ✅ · 2.6 budget mémoire 🟡 revue finale · 2.7 réputation ✅
- **Phase 3 (v3.0.0) — production** : 3.2 fuzzing ✅ · 3.3 simulation 11k ✅ · 3.4 auto-réparation ✅ · 3.6 observabilité ✅ · reste : 3.1 audit sécurité pro · 3.5 builds signés multi-plateforme · 3.7 canal de release + rollback · 3.8 doc opérateur · 3.9 décision ZK (SNARK réel ou retrait du claim)

---

## Contribuer

```bash
# 1. Fork + branche
git checkout -b feature/ma-feature
# 2. Les gates locales doivent passer AVANT toute PR :
cd core && cargo test --workspace --locked && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check
# 3. Commit conventionnel : type(scope): sujet
# 4. PR — un reviewer indépendant rejouera les gates ci-dessus.
```

Toute modification touchant `crypto/`, `network/`, `storage/`, `protocol/` ou `reputation/` exige des tests de contrat + chemins d'erreur + une preuve négative (le test doit échouer si on casse la logique qu'il prétend protéger).

---

## Licence

MIT — voir [LICENSE](LICENSE).
