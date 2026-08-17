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

### 🗑️ rust-core/ — supprimé (audit 2026-08)

`rust-core/` (workspace legacy) a été **audité puis supprimé**. Contrairement à ce que documentait ce README (« stub de 3 lignes »), il contenait en réalité :
- un `crypto-module` avec de la **cryptographie placeholder dangereuse** : `KeyPair::generate()` dérivait une clé publique aléatoire sans lien avec la clé privée, `SignedMessage::verify()` acceptait toute signature non nulle, `ZkProof::verify()` retournait toujours `true` — un tel code ne doit jamais être pris pour une implémentation réelle ;
- un `dtn-router` legacy, dupliqué et obsolète par rapport à `core/crates/dtn-router` ;
- un `llm-inference` stub (« Hello, world! ») sans équivalent dans `core/crates/llm-inference`.

Le workspace n'était référencé ni par la CI, ni par `core/` (qui possède ses propres crates), ni par le Dockerfile. Les implémentations réelles et auditées vivent dans `core/src/crypto/` (Ed25519 dalek, EncryptedEnvelope X25519+HKDF, verify_apk_signature), `core/src/` et `core/crates/`.

> ⚠️ **Note** : `core/` est le moteur actif. `rust-core/` a été supprimé — toute référence restante doit être ignorée ou nettoyée.

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
| Identité | Ed25519 keypair par nœud, rotation 6 h (forward secrecy) |
| Chiffrement | ChaCha20-Poly1305, ECDH X25519 + HKDF-SHA256 |
| Anti-spam | PoW CPU adaptatif + Web of Trust (réputation) — endossements **propagés dans le mesh** (Phase 1.2) |
| Transactions | ZK-Proofs asynchrones (commit différé) |
| DNS | TLD Handshake incensurables |
| Distribution APK | `core/src/update/` — annonce + manifeste signés Ed25519 (racine épinglée), transfert par chunks, `verify_apk_signature()` de bout en bout, **câblé dans le flux gossip** (Phase 1.1) |
| Confidentialité | **Padding de trafic** en seaux (256 B / 1 Ko / 4 Ko / 16 Ko) — **opérationnel sur le flux réseau** (Phase 1.3) : tout message émis est padé, tout message reçu est unpadé avant décodage |
| Économie batterie | Mode throttling adaptatif (`--battery-saver`), intervalle de sweep différé, publication espacée (`--battery-saver` ×6) |
| Anti-spam publication | Throttling adaptatif : 10 s (pair de confiance) / 120 s (non approuvé), multiplié par 6 en mode batterie |
| Résilience stockage | Base **SQLite** par nœud : persistance des événements reçus/publications, restauration complète au démarrage |
| Sharding géo-scopé | Stockage local filtré par **Geohash** (`my_geohash`, préfixe selon profil : Mobile 5 / Desktop 4 / Gateway 3) — les événements hors zone ne sont pas stockés |

### Distribution sécurisée des mises à jour (`core/src/update/`)

Protocole de mise à jour d'APK par le mesh (Audit #12/#13) : l'annonceur signe une `UpdateAnnouncement` (version + SHA-256) puis un manifeste canonique (`ApkManifest`, 80 octets) avec la clé racine épinglée ; le receveur vérifie chaque signature, télécharge l'APK par chunks (16 Kio), puis exécute `verify_apk_signature()` (racine épinglée + SHA-256 du fichier entier) avant toute installation. Un APK falsifié, une version non supérieure, un manifeste non lié à l'annonce ou une racine inconnue sont rejetés.

**Phase 1.1 — opérationnel dans le flux réel** : le protocole est câblé dans le gossip (`OndeMessageType::UpdateAnnounce/Manifest/Chunk/Request`, codes 9–12 — le format wire des types existants reste stable). Le blob signé (base64) circule dans `MeshEvent.content`, les métadonnées (`root_sig`, `version`, `peer`, `index`, `total`, `to`) dans `tags` au format `k=v`. `Node::announce_update(version, apk, timestamp)` signe l'annonce **et** le manifeste avec la clé racine de distribution (`NodeConfig::update_root_seed`) et diffuse dans le gossip ; `Node::handle_incoming_update(event)` pilote la machine à états côté receveur (annonce → requête manifeste → manifeste → requêtes chunks → assemblage → vérification → installation) et sert les requêtes côté annonceur (le PoW adaptatif de la réputation est conservé : un émetteur de confiance diffuse avec difficulté 0). Voir la doc du module pour le diagramme de flux complet.

### Web of Trust décentralisé — propagation des endossements (`core/src/reputation/`)

**Phase 1.2 — opérationnel dans le flux réel** : les endossements ne sont plus **locaux** (appel direct `ReputationSystem::endorse`) : ils sont désormais **propagés dans le mesh**. Un `Endorsement` (`endorser`, `endorsed`, `timestamp`) est sérialisé en JSON puis base64 dans `MeshEvent.content` et diffusé sous le type `OndeMessageType::Endorsement` (code 13 — le format wire des types existants reste stable). `Node::endorse(peer_pubkey)` applique l'endossement localement (réutilise la logique qualifiée `endorse` : anti-self, anti-doublon, seuil d'endosseur — sans la dupliquer), signe l'événement avec l'identité du nœud (Ed25519 sur l'ID canonique) et le diffuse dans le gossip avec le PoW adaptatif (endosseur de confiance → difficulté 0). `Node::handle_incoming_endorsement(event)` vérifie la signature de l'endosseur, intègre via `ReputationSystem::apply_remote_endorsement` (mêmes règles que `endorse` : endosseur non de confiance, self ou doublon → rejeté), puis **relaie** l'endossement vers les pairs qui ne l'ont pas encore reçu (tracking « livré par pair » du gossip) — la cascade propage l'endossement à tout le mesh, et chaque receveur intègre les endossements reçus dans sa vue du Web of Trust. Un nœud atteint le statut « de confiance » après `REQUIRED_ENDORSEMENTS` endossements qualifiés de nœuds de confiance, exactement comme en local.

### Confidentialité — padding de trafic (`core/src/crypto/`)

**Phase 1.3 — opérationnel sur le flux réseau** : `TrafficPadding` n'est plus un primitif inutilisé (dead code) — il est câblé au **point de sérialisation** du gossip, le plus centralisé du flux réel (`GossipProtocol` + `Node` ; le trait `MeshTransport` n'est pas encore branché au gossip, envelopper `send` aurait laissé le padding inopérant). Tout `MeshEvent` émis vers un pair est sérialisé en format wire binaire compact puis **padé au seau** (`MeshEvent::to_wire_bytes` : 256 B / 1 Ko / 4 Ko / 16 Ko — la taille observée est toujours un seau, jamais la taille réelle) ; tout octet reçu est **unpadé avant décodage et validation** (`MeshEvent::from_wire_bytes`). Le receveur tolère les messages non padés (aucun zéro de fin → retour identique) et `unpad` est idempotent ; `pad` ne tronque jamais un message plus gros que le seau maximal (16 Ko) — tronquer serait une perte de données silencieuse. Le flux e2e `gossip_sync` achemine désormais par ce wire padé/unpadé.

---

## 🧪 Tests

### Suite du moteur (`core/`) — 153 tests, 0 échec

```bash
# Tous les tests (workspace core/)
cd core && cargo test --workspace

# Résultats : 153 tests, 0 échec
# onde-core        : 115 tests ✅ Crypto, Network, Protocol, Storage, Update, Node, AI, Reputation
# dtn-router       :  7 tests ✅ Store-and-forward, broadcast, priorités, TTL
# llama-bind       :  5 tests ✅ Sélection de modèle, génération mock, quantification
# whisper-stt      :  4 tests ✅ Création d'engine, transcription mock
# zim-parser       :  3 tests ✅ Extraction HTML, catégories, URL ZIM
# llm-inference    :  3 tests ✅ Inférence locale, auto-sélection de modèle
# integration_e2e  : 16 tests ✅ Scénarios end-to-end complets
```

Les tests ajoutés en Phase 1.3 couvrent le **padding de trafic opérationnel** : tailles de seaux (`pad` 100 B → 256 B, 2000 B → 4096 B, seau maximal 16 384 B pour 20 000 B **sans troncature**), round-trip `unpad(pad(x)) == x` sur 5 tailles (1, 100, 1000, 5000, 30000), `unpad` tolérant (message non padé → identique) et idempotent (`unpad(unpad(pad(x))) == unpad(pad(x))`), message vide → 256 B sans panique, format wire `MeshEvent::to_wire_bytes`/`from_wire_bytes` (round-trip champ par champ, entrée vide/tronquée → erreur propre), et le test e2e `test_traffic_padding_wire_two_nodes` (A publie une alerte de 100 B → **256 B observés sur le fil** → B décode un contenu identique via le helper `gossip_sync` qui achemine par le wire padé).

Les 4 tests ajoutés en Phase 1.2 couvrent : la propagation des endossements entre nœuds (`test_endorsement_propagation_three_nodes`), l'application d'un endossement reçu du réseau (`apply_remote_endorsement` : signature, anti-self, anti-doublon, seuil d'endosseur, promotion), le jeu de relai (`pending_endorsements`) et la stabilité du code wire du nouveau type `Endorsement` (code 13, sans renumérotation).

Les 2 tests ajoutés au dernier audit couvrent : l'application du sharding Geohash au stockage local (`test_store_applies_geohash_sharding`) et le throttling adaptatif de publication (`test_node_publish_throttle`).

Les tests du protocole de mise à jour (`core/src/update/`) couvrent : flux complet annonce → manifeste → chunks → vérification → installation, rejet des APK falsifiés, rejet des signatures de racine inconnue, rejet des versions non supérieures, rejet des manifestes non liés à l'annonce, bornes des chunks, et non-contournabilité par les métadonnées non signées.

Le câblage gossip de la Phase 1.1 est couvert par les tests e2e `test_update_flow_between_two_nodes` (annonce → requêtes → manifeste → chunks → assemblage → vérification → installation, APK identique byte-à-byte, rejet des versions non supérieures) et `test_update_rejects_tampered_apk` (un APK falsifié est rejeté à l'assemblage et le transfert empoisonné est purgé), sur le modèle `test_multi_node_gossip` (deux `Node` + `add_event` + `get_pending_for_peer`).

La propagation WoT de la Phase 1.2 est couverte par `test_endorsement_propagation_three_nodes` (A endosse B, l'endossement est diffusé puis **relayé** A → B → C, la réputation de B monte chez C, un endossement d'un nœud non de confiance est ignoré, un doublon est ignoré, et après 3 endossements qualifiés C considère B comme de confiance), sur le même modèle gossip que `test_multi_node_gossip`.

### rust-core/ — supprimé

`rust-core/` a été supprimé lors de l'audit de sécurité (crypto placeholder dangereuse + code legacy dupliqué, voir section dédiée plus haut).

### Exécution du Nœud

```bash
# Nœud ONDE (daemon, arrêt par Ctrl+C)
cd core
cargo run --bin onde_node -- --type mobile --name "MyNode" --geohash u09tunq
# Options : --type <mobile|desktop|gateway> | --name <nom> | --geohash <geohash> | --battery-saver | --help
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
- ✅ 153 tests unitaires + intégration, 0 échec
- ✅ Simulation réseau 11k nœuds validée (v0.2.5)
- ✅ UI HTML AMOLED Black standalone
- ✅ Bridge Tauri fonctionnel : l'UI appelle le noyau (démarrage nœud, publication alerte/entraide, flux) via les commandes Tauri ; fallback démo navigateur hors Tauri
- ✅ Persistance SQLite des événements (restauration au démarrage) + sharding géo-scopé Geohash

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