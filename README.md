# ⧫ ONDE — Réseau de Résilience Citoyen

> **Application cross-platform de réseau mesh hors-ligne : social, financier et intelligent.**

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021-orange.svg)](https://www.rust-lang.org)
[![Tauri](https://img.shields.io/badge/built%20with-Tauri-black.svg)](https://tauri.app)

---

## 📡 Vue d'Ensemble

**ONDE** est une infrastructure de survie numérique globale : réseau maillé, social, financier et intelligent fonctionnant **100% hors-ligne**.

### Fonctionnalités Clés

| Module | Description |
|---|---|
| 🔄 **Réseau Mesh** | Wi-Fi Aware, BLE, LoRa (Meshtastic), Ethernet Bridge. Routage DTN store-and-forward |
| 📝 **Social Text-Only** | Protocole Nostr. Flux d'alertes 280 car. + entraide hiérarchisée. Zéro image |
| 🎙️ **Voix Asynchrone** | Mémos vocaux Opus 8kbps transitant via DTN, avec transcription STT automatique |
| 🧠 **IA Locale** | PocketPal mobile (Qwen 0.8-9B quantized) + Super-Oracles desktop (70B+ via RPC) |
| 🗺️ **Cartes Offline** | MBTiles vectorielles + positionnement Geohash radar |
| 📚 **Encyclopédie** | Lecteur ZIM (Wikipédia hors-ligne) |
| 💰 **Finance ZK** | Transactions asynchrones ZK-Proofs type Mina. Push blockchain quand internet dispo |
| 📁 **Méga-Archives** | IPFS seeder desktop : APK, ZIM, modèles IA |
| 🔐 **Sécurité** | Ed25519, ChaCha20-Poly1305, PoW antispam CPU, Handshake DNS |

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
├─────────────────────────────────────────────────────┤
│             Platform Abstractions                    │
│  Android/iOS ◄► Desktop (Win/Mac/Linux)             │
└─────────────────────────────────────────────────────┘
```

### Structure du Dépot

```
onde/
├── Dockerfile.dev              # Env dev: Rust, Python, Android SDK
├── docker-compose.yml           # Dev + Simulation services
├── .devcontainer/               # VS Code remote container
├── simulation/                  # PHASE 1 — SimPy network sim
│   ├── mesh_sim.py             # 10k-500k nodes simulation
│   └── results/                 # JSON reports
├── core/                        # PHASE 2 — Rust core engine
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs              # Main exports
│   │   ├── bin/node.rs         # CLI node daemon
│   │   ├── network/mod.rs      # Mesh transport + Yggdrasil
│   │   ├── protocol/mod.rs     # Nostr events + PoW + Gossip
│   │   ├── crypto/mod.rs       # Ed25519 + ZK transactions
│   │   ├── storage/mod.rs       # ZIM + MBTiles + IPFS
│   │   ├── ai/mod.rs           # AI Engine manager
│   │   └── node/mod.rs         # Node orchestrator
│   └── crates/
│       ├── dtn-router/         # Store-and-Forward routing
│       └── llm-inference/      # PocketPal + Oracle RPC
├── ui/                          # PHASE 3 — Tauri application
│   ├── src/
│   │   └── index.html          # AMOLED Black UI (standalone)
│   ├── src-tauri/
│   │   ├── Cargo.toml
│   │   ├── tauri.conf.json
│   │   └── src/main.rs
│   └── web/package.json
└── README.md
```

---

## 🚀 Démarrage Rapide

### Avec Docker (Recommandé)

```bash
# Build l'image de dev
docker compose build

# Entrer dans le conteneur
docker compose run dev bash

# Lancer la simulation
python3 simulation/mesh_sim.py

# Build le core Rust (dans le conteneur)
cd core && cargo test

# Run nœud standalone
cargo run --bin onde_node -- --type mobile --name "MonNoeud"
```

### Sans Docker

```bash
# Requiert: Rust 1.75+, Python 3.10+
pip install simpy numpy

# Simulation
python3 simulation/mesh_sim.py

# Core Rust
cd core && cargo test
cd core && cargo run --bin onde_node
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

# Sortie typique :
# === ONDE MESH SIMULATION ===
# [t=   3600s] Envoyés: 15,234 | Délivrés: 12,891 (84.6%)
# DTN hops: 3,456 | PoW OK: 14,890 | Tx committed: 892
# ✅ Simulation terminée avec succès!
```

### Technologies simulées :

| Tech | Portée | Bandwidth | Usage |
|------|--------|-----------|-------|
| Wi-Fi Aware | 200m | 50 Mbps | Communication principale mobile |
| BLE | 50m | 2 Mbps | Proximité directe |
| LoRa | 5km | 50 kbps | Longue distance, alerts |
| Ethernet | ~1km | 1 Gbps | Ponts desktop vers filaire |

---

## 🔧 Modules Core Rust

### Network Layer

```rust
// Multi-transport manager
let mut transport = MultiTransport::new();
transport.add_transport(Box::new(WifiAwareTransport::new()));
transport.add_transport(Box::new(BluetoothTransport::new()));

// Send via best available
transport.send_best("peer-addr", &data).await?;

// Yggdrasil IPv6 mesh addressing
let addr = YggdrasilAddress::new("my-node");
let ipv6 = addr.generate_ipv6();  // "200:xxxx:..."
```

### Protocol Layer

```rust
// Nostr-style event with PoW
let mut event = MeshEvent::new(
    pubkey,
    OndeMessageType::Alert,
    "⚠️ Urgence secteur Nord".to_string(),
    vec![],
);
event.compute_pow(100_000);  // CPU PoW
gossip.add_event(event);
```

### Crypto & ZK

```rust
// Identity
let identity = Identity::generate();
let signature = identity.sign(data);

// ZK Transaction (offline async)
let tx = ZkTransaction::new("alice", "bob", 1_000_000, nonce);
pool.submit(tx)?;
pool.commit_pending(100); // Push when internet available
```

### AI Engine

```rust
// PocketPal on mobile (auto-select model by RAM)
let engine = PocketPalEngine::new(4096);  // 4GB RAM available
let response = engine.infer("Premiers secours RCP", 256).await;

// Desktop Oracle RPC
let oracle = OracleRpcServer::new(8080);
oracle.start().await?;
```

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

## 🗺️ Roadmap

### Version actuelle : 0.1.0 (Prototype)
- ✅ Core Rust: network, protocol, crypto, storage, AI
- ✅ SimPy simulation network
- ✅ UI AMOLED Black standalone HTML
- ✅ Tauri desktop skeleton
- ✅ DTN router (store-and-forward)
- ✅ Yggdrasil addressing
- ✅ ZK transaction pool
- ✅ PocketPal AI inference

### Version 0.2.0 (À venir)
- [ ] Intégration llama.cpp (GGML FFI)
- [ ] Wi-Fi Aware native Android
- [ ] IPFS-lite node complet
- [ ] Whisper.cpp STT
- [ ] ZIM file parser complet

### Version 1.0.0 (Objectif)
- [ ] Production builds: APK, IPA, EXE, DMG, AppImage
- [ ] 802.11s kernel module AOSP
- [ ] Mina Protocol integration
- [ ] Handshake HNS resolution
- [ ] Meshtastic LoRa integration

---

## 🧪 Tests

```bash
# Core Rust tests
cd core && cargo test

# Expected results:
# running 12 tests
# test crypto::tests::test_identity_sign_verify ... ok
# test crypto::tests::test_zk_transaction_creation ... ok
# test network::tests::test_transport_ranges ... ok
# test protocol::tests::test_alert_size_limit ... ok
# test storage::tests::test_geohash ... ok
# test node::tests::test_node_creation ... ok
# ... all passed
```

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