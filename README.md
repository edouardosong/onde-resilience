# ⧫ ONDE — Réseau de Résilience Citoyen

> **Application cross-platform de réseau mesh hors-ligne : social, financier et intelligent.**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
[![Tests](https://img.shields.io/badge/tests-79%20passing-brightgreen.svg)]()
[![Version](https://img.shields.io/badge/version-0.2.0-blue.svg)]()
[![Build](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![Release](https://img.shields.io/github/v/release/edouardosong/onde-resilience?label=latest)](https://github.com/edouardosong/onde-resilience/releases)

---

## ⚠️ Statut du projet

**Prototype** — code non audité, non durci, **pas prêt pour la production**.
Données de démo explicites (`load_demo` / `register_demo_seeds`).
Les modules marqués **Prototype** sont fonctionnels en tests mais non durcis.

---

## 📡 Vue d'Ensemble

**ONDE** est une infrastructure de survie numérique globale : réseau maillé, social, financier et intelligent fonctionnant **100% hors-ligne**.

### Fonctionnalités Clés

| Module | Description | Statut |
|---|---|---|
| 🔄 **Réseau Mesh** | Wi-Fi Aware, BLE, LoRa (Meshtastic), Ethernet Bridge. Routage DTN store-and-forward | Prototype (tests verts) |
| 📝 **Social Text-Only** | Protocole Nostr. Flux d'alertes 280 car. + entraide hiérarchisée. Zéro image | Prototype |
| 🎙️ **Voix Asynchrone** | Mémos vocaux Opus 8kbps transitant via DTN, avec transcription STT automatique | Prototype |
| 🧠 **IA Locale** | PocketPal mobile (Qwen 0.8-9B quantized) + Super-Oracles desktop (70B+ via RPC) | Prototype (binding llama, démo) |
| 🗺️ **Cartes Offline** | MBTiles vectorielles + positionnement Geohash radar | Prototype (demo data via load_demo) |
| 📚 **Encyclopédie** | Lecteur ZIM (Wikipédia hors-ligne) | Prototype (demo data via load_demo) |
| 💰 **Finance ZK** | Transactions asynchrones ZK-Proofs type Mina. Push blockchain quand internet dispo | Mock (ZkProof::verify non implémenté) |
| 📁 **Méga-Archives** | IPFS seeder desktop : APK, ZIM, modèles IA | Prototype (demo data via register_demo_seeds) |
| 🔐 **Sécurité** | Ed25519, ChaCha20-Poly1305, PoW antispam CPU, Handshake DNS | Prototype |

---

## 🏗️ Architecture

### Multi-Platform Core

```
┌─────────────────────────────────────────────────────┐
│                   UI Layer (Tauri)                    │
│  React + HTML/CSS AMOLED Black + Navigation         │
├─────────────────────────────────────────────────────┤
│                Bridge Layer (Rust)                   │
│  Tauri Commands → onde_core                         │
├─────────────────────────────────────────────────────┤
│                   Core Engine (Rust)                  │
│  ┌──────────┬─────────┬─────────┬─────────────┐    │
│  │ Network  │Protocol │ Crypto  │   Storage   │    │
│  │ Mesh/DTN │ Nostr   │ Ed25519 │ ZIM/MBTiles │    │
│  │ Yggdrasil│ PoW     │ ChaCha  │ IPFS Seeder │    │
│  └──────────┴─────────┴─────────┴─────────────┘    │
│  ┌──────────────────────────────────────────────┐   │
│  │  AI Engine (llm-inference crate)             │   │
│  │  PocketPal (mobile)  ◄►  OracleRPC (desk)   │   │
│  └──────────────────────────────────────────────┘   │
│  ┌──────────────────────────────────────────────┐   │
│  │  DTN Router (dtn-router crate)               │   │
│  │  Bundle queues, custody transfer, routing   │   │
│  └──────────────────────────────────────────────┘   │
├─────────────────────────────────────────────────────┤
│             Platform Abstractions                    │
│  Android/iOS ◄► Desktop (Win/Mac/Linux)             │
└─────────────────────────────────────────────────────┘
```

### Structure du Projet

```
onde-resilience/
├── 📄 README.md                 # Ce fichier - Documentation complète
├── 🐳 Dockerfile.dev            # Env dev: Rust, Python, Android SDK
├── 🐳 docker-compose.yml        # Dev + Simulation services
├── 🔧 .devcontainer/            # VS Code remote container
├── 🧪 simulation/               # PHASE 1 — SimPy network sim (11k nœuds)
│   ├── mesh_sim.py              # Simulation réseau mesh DTN
│   └── results/                 # Rapports JSON des simulations
├── 🦀 core/                     # MOTEUR PRINCIPAL — workspace Cargo actif
│   ├── Cargo.toml               # Workspace manifest (onde_core + onde_node + 5 crates)
│   ├── src/                     # onde_core: network, protocol, crypto, storage, node, ai
│   ├── bin/node.rs              # onde_node — binaire du nœud
│   └── crates/                  # dtn-router, zim-parser, llm-inference,
│                                # llama-bind, whisper-stt
├── 🗑️ rust-core/                # STUB / legacy (placeholder Cargo, non utilisé)
│   ├── Cargo.toml               # Workspace placeholder
│   └── src/main.rs              # "Hello, world!" (3 lignes) — à supprimer ou fusionner
├── 🎨 ui/                       # PHASE 3 — Interface utilisateur
│   ├── src/
│   │   └── index.html           # UI AMOLED Black standalone (40KB)
│   ├── src-tauri/               # Application Tauri cross-platform
│   │   ├── Cargo.toml
│   │   ├── tauri.conf.json
│   │   └── src/main.rs
│   └── web/package.json
└── 📜 LICENSE                   # MIT License
```

---

## 🚀 Démarrage Rapide

### Avec Docker (Recommandé)

```bash
# Build l'image de dev
docker compose build

# Entrer dans le conteneur
docker compose run dev bash

# Lancer la simulation (11k nœuds)
python3 simulation/mesh_sim.py

# Build le moteur (dans le conteneur)
cd core && cargo test --workspace

# Run le nœud ONDE
./target/release/onde_node
```

### Sans Docker

```bash
# Requiert: Rust 1.75+, Python 3.10+
pip install simpy numpy

# Simulation réseau
python3 simulation/mesh_sim.py

# Moteur (core/)
cd core
cargo test --workspace
cargo build --release

# Exécuter le nœud
./target/release/onde_node
```

### UI Standalone

L'interface est un fichier HTML autonome — ouvrez-le directement dans un navigateur :

```bash
# Sur n'importe quel navigateur
open onde/ui/src/index.html
```

---

## 📊 Simulation Réseau

Le simulateur (`mesh_sim.py`) valide la topologie face aux flux :

```bash
# Configuration par défaut: 10k mobile + 1k desktop bridges
python3 simulation/mesh_sim.py

# Sortie typique (v0.2.5 - 11k nœuds) :
# === ONDE MESH SIMULATION ===
# [t=   3600s] Envoyés: 15,894 | Délivrés: 5,237,664 (multi-hop DTN)
# DTN hops: 22,896 | PoW OK: 7,731 | Tx committed: 3,141
# ✅ Simulation terminée avec succès! (165.7s réels)
```

### Technologies simulées :

| Tech | Portée | Bandwidth | Usage |
|------|--------|-----------|-------|
| Wi-Fi Aware | 200m | 50 Mbps | Communication principale mobile |
| BLE | 50m | 2 Mbps | Proximité directe |
| LoRa | 5km | 50 kbps | Longue distance, alerts |
| Ethernet | ~1km | 1 Gbps | Ponts desktop vers filaire |

### Résultats clés v0.2.5 :

- **11 000 nœuds** simulés (10k mobiles + 1k bridges)
- **1h de simulation** en 165.7s réels
- **15 894 messages** envoyés
- **5.2M délivrances** via routage multi-hop DTN
- **7 731 PoW** réussis (antispam)
- **3 141 transactions** ZK commitées

---

## 🔧 Modules du moteur (`core/`)

### 📦 DTN Router (`core/crates/dtn-router` crate)

Routage DTN store-and-forward : buffers par nœud, priorités, rencontre opportuniste, expiration TTL.

```rust
// Buffer store-and-forward avec priorités (0 = urgence max)
let router = DtnRouter::new(1000);

// Stocker un message (broadcast ou unicast)
router.store("alice", msg).await;

// Rencontre opportuniste entre deux nœuds → transfert
let (to_a, to_b) = router.encounter("alice", "bob").await;

// Stats de livraison
let stats = router.stats().await;
```

**Fonctionnalités :**
- ✅ Buffer par nœud avec priorités (0 = urgence max)
- ✅ Transfert opportuniste à la rencontre (store-and-forward)
- ✅ Diffusion broadcast avec déduplication par pair
- ✅ Expiration TTL et statistiques de livraison
- ✅ Tests unitaires

### 🔐 Crypto & Protocole (`core/src/crypto`, `core/src/protocol`)

Identités Ed25519 + X25519, chiffrement de bout en bout ChaCha20-Poly1305 (HKDF-SHA256), événements signés à ID canonique avec PoW antispam, transactions ZK asynchrones (mock).

```rust
// Identité Ed25519 + X25519
let identity = Identity::generate();
let sig = identity.sign(b"message");
assert!(identity.verify(b"message", &sig));

// Chiffrement de bout en bout (X25519 ECDH + ChaCha20-Poly1305)
let envelope = EncryptedEnvelope::encrypt(b"secret", &alice, &bob.x25519_public_key_bytes())?;
let plain = EncryptedEnvelope::decrypt(&envelope, &bob)?;

// Événement signé (Nostr-style) + PoW antispam
let mut event = MeshEvent::new_signed(&identity, OndeMessageType::Alert, content, vec![]);
event.compute_pow(100_000);
assert!(event.validate().is_ok());

// Transactions ZK asynchrones (mock)
let tx = ZkTransaction::new("alice", "bob", 1_000_000, 0);
pool.submit(tx)?;
```

**Fonctionnalités :**
- ✅ Identités Ed25519 (signature/vérification) + X25519 (ECDH)
- ✅ Chiffrement de bout en bout réel : X25519 + HKDF-SHA256 + ChaCha20-Poly1305
- ✅ Événements signés à ID canonique, PoW antispam CPU
- ⚠️ ZK proofs : **mock** (`ZkProof::verify` non implémenté — SNARK réel à venir)
- ✅ Tests unitaires

### 🤖 IA Locale (`core/crates/llm-inference` + `core/crates/llama-bind`)

Prototype : sélection de modèle selon la RAM, oracle RPC desktop, bindings llama.cpp et STT Whisper en mode mock.

```rust
// Sélection automatique du modèle selon la RAM disponible
let engine = PocketPalEngine::new(available_ram_mb);
let resp = engine.infer("question", 256).await;

// Oracle RPC (nœud desktop)
let server = OracleRpcServer::new(8080);
server.process(req).await;

// Binding llama.cpp (mock)
let mut ctx = LlamaContext::new(model, GenerationConfig::default());
ctx.load("qwen2.5-7b.gguf")?;
ctx.generate("Premiers secours ?").await?;
```

**Fonctionnalités :**
- ✅ Sélection de modèle selon la RAM (Qwen 0.5B → 7B, Llama 70B)
- 🔄 Binding llama.cpp **mock** (GGML réel non implémenté)
- 🔄 STT Whisper **mock** (transcription réelle non implémentée)
- ✅ Tests unitaires

### 🗑️ rust-core/ (stub legacy)

`rust-core/` est un **stub** : workspace Cargo placeholder + `src/main.rs` de 3 lignes (`println!("Hello, world!")`). Aucun code métier. À supprimer ou fusionner dans `core/`.

> ⚠️ **Note** : `core/` est le moteur actif. `rust-core/` est un stub legacy (3 lignes) à supprimer ou fusionner dans `core/`.

---

## 🎨 Interface Utilisateur

Thème **AMOLED Black pur** (`#000000`) avec accent vert néon.

### Pages :
1. **⚡ Alertes** — Flux public d'alertes 280 car. et d'entraide
2. **📡 Radar** — View mesh radar + position Geohash
3. **🧠 IA** — Chat PocketPal local
4. **💰 Wallet** — Portefeuille ZK hors-ligne
5. **📚 Wiki** — Encyclopédie ZIM offline
6. **📱 P2P** — Partage fichiers par QR Code

### Ouverture rapide :
```bash
# Dans un navigateur (Chrome, Firefox, Edge)
open onde/ui/src/index.html
```

---

## 📦 Build Cross-Platform

### Desktop (Tauri)

```bash
cd ui/src-tauri
cargo tauri build

# Outputs :
# Linux   → src-tauri/target/release/bundle/appimage/onde.AppImage
# Windows → src-tauri/target/release/bundle/nsis/onde-setup.exe
# macOS   → src-tauri/target/release/bundle/dmg/onde.dmg
```

### Android

```bash
# Via Tauri Android
cd ui/src-tauri
cargo tauri android init
cargo tauri android build

# Output: Onde.apk
```

### iOS

```bash
cargo tauri ios init
cargo tauri ios build
```

---

## 🔒 Sécurité

| Couche | Mécanisme |
|--------|-----------|
| Identité | Ed25519 keypair par nœud |
| Chiffrement | ChaCha20-Poly1305 |
| Anti-spam | PoW CPU adaptatif (difficulty 2-8) |
| Transactions | ZK-Proofs asynchrones (commit différé) |
| DNS | TLD Handshake incensurables |

---

## 🧪 Tests

### Suite du moteur (`core/`) — 79 tests, 0 échec

```bash
# Tous les tests (workspace core/)
cd core && cargo test --workspace

# Résultats : 79 tests, 0 échec
# onde-core        : 46 tests ✅ Crypto, Network, Protocol, Storage, Node, AI
# dtn-router       :  6 tests ✅ Store-and-forward, broadcast, priorités, TTL
# llama-bind       :  5 tests ✅ Sélection de modèle, génération mock, quantification
# whisper-stt      :  4 tests ✅ Création d'engine, transcription mock
# zim-parser       :  3 tests ✅ Extraction HTML, catégories, URL ZIM
# llm-inference    :  3 tests ✅ Inférence locale, auto-sélection de modèle
# integration_e2e  : 12 tests ✅ Scénarios end-to-end complets
```

### rust-core/ (stub)

`rust-core/` ne contient aucun test métier (placeholder `src/main.rs` de 3 lignes).

### Exécution du Nœud

```bash
# Nœud ONDE (daemon, arrêt par Ctrl+C)
cd core
cargo run --bin onde_node -- --type mobile --name "MyNode"
# Options : --type <mobile|desktop> | --name <nom> | --help
```

---

## 📦 Builds et Releases

### Compilation

```bash
# Debug build (avec symbols)
cd core && cargo build

# Release build (optimisé)
cd core && cargo build --release

# Outputs:
# target/debug/onde_node
# target/release/onde_node
```

### Desktop (Tauri)

```bash
cd ui/src-tauri
cargo tauri build

# Outputs :
# Linux   → src-tauri/target/release/bundle/appimage/onde.AppImage
# Windows → src-tauri/target/release/bundle/nsis/onde-setup.exe
# macOS   → src-tauri/target/release/bundle/dmg/onde.dmg
```

### Android

```bash
# Via Tauri Android
cd ui/src-tauri
cargo tauri android init
cargo tauri android build

# Output: Onde.apk
```

### iOS

```bash
cargo tauri ios init
cargo tauri ios build
```

---

## 🗺️ Roadmap

### 🔄 Version actuelle : 0.2.0 — Prototype

**Implémenté (prototype, tests verts) :**
- ✅ Workspace Cargo actif : onde_core + onde_node + 5 crates (dtn-router, zim-parser, llm-inference, llama-bind, whisper-stt)
- ✅ Chiffrement de bout en bout réel (X25519 + HKDF + ChaCha20-Poly1305), événements signés, PoW antispam
- ✅ Routage DTN store-and-forward (buffers priorisés, broadcast avec déduplication, TTL)
- ✅ 79 tests unitaires + intégration, 0 échec
- ✅ Simulation réseau 11k nœuds validée (v0.2.5)
- ✅ UI HTML AMOLED Black standalone

**Manques avant toute production :**
- [ ] Audit de sécurité professionnel et fuzzing tests
- [ ] ZK proofs réels (SNARK) — actuellement mock
- [ ] Binding llama.cpp réel (GGML) — actuellement mock
- [ ] STT Whisper réel — actuellement mock
- [ ] Lecture réelle des fichiers ZIM/MBTiles — actuellement données de démo

### 🔄 Version 2.0.0 (Objectif Q1-Q2 2025)

- [ ] Bindings Python via PyO3 pour intégration simulation ↔ core Rust
- [ ] Intégration libp2p complète (remplacement du mock DTN)
- [ ] Audit de sécurité professionnel et fuzzing tests
- [ ] Containers Docker production-ready
- [ ] Helm charts Kubernetes pour déploiement cloud
- [ ] CI/CD automatisé (GitHub Actions)
- [ ] Transcription vocale Whisper opérationnelle
- [ ] Inference Llama avec modèles bundlés (Qwen GGUF)

### 🎯 Version 3.0.0 (Objectif Q3-Q4 2025)

- [ ] Production builds: APK Android, IPA iOS, EXE Windows, DMG macOS, AppImage Linux
- [ ] Module kernel 802.11s pour mesh Wi-Fi natif AOSP
- [ ] Intégration Mina Protocol pour transactions ZK blockchain
- [ ] Résolution DNS Handshake (HNS) pour TLD incensurables
- [ ] Intégration Meshtastic LoRa officielle
- [ ] Modèles IA bundlés dans l'application (Qwen 7B + Whisper small)
- [ ] Fichiers ZIM bundlés (Wikipedia offline complet)
- [ ] Lecteur ZIM intégré dans l'UI
- [ ] Cartes MBTiles vectorielles offline
- [ ] Application mobile Tauri complète (Android + iOS)

### 🚀 Vision Long Terme (2026+)

- [ ] Déploiement massif : 1M+ nœuds simulés
- [ ] Partenariats ONG humanitaires
- [ ] Certification sécurité gouvernementale
- [ ] Réseau mesh communautaire auto-suffisant
- [ ] DAO de gouvernance ONDE

---

## 📄 Licence

MIT License. Voir le fichier LICENSE.

---

## 🤝 Contribuer

```bash
# 1. Fork le dépôt
# 2. Créez votre branche (git checkout -b feature/amazing-feature)
# 3. Commit (git commit -m 'Add amazing feature')
# 4. Push (git push origin feature/amazing-feature)
# 5. Ouvrez une Pull Request
```

---

> **ONDE** — Parce que la résilience commence par la connexion. ⧫