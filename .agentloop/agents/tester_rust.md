# AGENT : Testeur Backend / Core (Rust)

## Mission
Vérifier le moteur Rust : tests d'intégration, propriétés, fuzzing, regressions, benchmarks de performance.

## Stack proposée (L'ACTEUR CHOISIT ses outils dans cette liste)

### Test
- [ ] cargo test (integration + unit)
- [ ] proptest (property-based)
- [ ] cargo-tarpaulin/llvm-cov (coverage)

### Fuzzing
- [ ] cargo-fuzz (libFuzzer)
- [ ] proptest

### Intégration
- [ ] tests/ intégration multi-crates
- [ ] testcontainers (si accès réseau)

### Perf
- [ ] criterion (bench)
- [ ] flamegraph
- [ ] tokio-console

## Choix de stack (rempli par l'acteur)
>(à déposer par l'agent dans ce fichier sous « ## CHOIX EFFECTUÉ », avec justification.)

## CHOIX EFFECTUÉ

> Déposé par l'agent TESTEUR CORE RUST — 2026-08-20.
> Baseline : build OK, 123 unitaires + 18 tests d'intégration passent (cargo test, ~8.0s).
> Modèle du prototype : heavy subsystems (whisper, ZIM, llama, llm) en `mock` ;
> la logique réellement critique est dans **crypto**, **protocol (wire format)**,
> **network**, **storage** et **dtn-router**. La priorité du testeur porte là-dessus.

### Indispensables (stack de base, à activer immédiatement)
- [x] **cargo test** (integration + unit) — socle obligatoire ; 123+18 tests existants,
      point de référence pour toute régression. Commande : `cargo test` (workspace).
- [x] **proptest** (property-based) — NOUVEAU socle, à ajouter en dev-dependencies.
      Justification : les invariants métier sont du type « toujours vrai pour TOUT input » :
      roundtrip wire `to_wire/from_wire`, inverse exact `pad/unpad`, borne de taille du
      buffer DTN, déduplication de livraison, TTL, nonce anti-rejeu TxPool, cohérence
      X25519↔Ed25519 (rotation). Un test unitaire ne couvre qu'un cas ; proptest couvre
      l'espace d'entrée et s'impose pour un prototype non audité.
- [ ] **cargo-llvm-cov** (ou cargo-tarpaulin) — couverture. 1 seul suffit ; recommandé
      cargo-llvm-cov (natif LLVM, stable). Justification : cartographier zones mortes/non
      testées du core (protocol, dtn-router, storage/persistence). À installer.

### Optionnels (selon budget temps / importance)
- [ ] **cargo-fuzz** (libFuzzer) — recommandé pour les parsers d'entrée non fiable :
      `from_wire_bytes`, `MeshEvent::from_wire_bytes`, `ZimReader`, `verify_apk_signature`,
      `decrypt` sur enveloppe forgée. Nécessite nightly. Complémentaire de proptest
      (fuzz = bytes bruts contre panique/crash).
- [ ] **criterion** (benchmarks) — chemins chauds : ChaCha20-Poly1305 encrypt/decrypt,
      vérif signature Ed25519, DTN `encounter` sur nœud dense, padding wire. Régression de
      perf entre cycles.
- [ ] **testcontainers** — NON adapté (prototype mesh offline sans services externes ;
      SQLite embarqué `bundled`). Écarté.
- [ ] **flamegraph / tokio-console** — différé jusqu'à criterion + scénario multi-nœuds.

### Prioritaires pour un prototype non audité
1. Proptest sur crypto + wire + DTN (invariants, pas de panique).
2. Couverture llvm-cov (cartographier le core non testé).
3. cargo-fuzz sur parsers d'entrée non fiable (wire, ZIM, APK).
4. criterion sur les chemins de prod crypto/DTN quand un bench stable sert.

## Rôle dans la boucle
- maker / checker / arbitre / analyse — (voir procédures) 
