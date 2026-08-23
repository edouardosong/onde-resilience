# AGENT : Développeur Core Rust

## Mission
Implémenter et maintenir le moteur Rust (network DTN, protocole nostr, crypto, storage, llm-inference, whisper-stt) en TDD.

## Stack proposée (L'ACTEUR CHOISIT ses outils dans cette liste)

### Build/test (indispensables — workflow quotidien)
- [x] cargo build / cargo test (TDD : boucle maker/checker, baseline verte 123 unit + 18 intégration)
- [x] cargo clippy -- -D warnings (porte strict avant tout commit)
- [x] cargo fmt (normalisation, diff propres pour le checker)
- [x] rust-analyzer (push JSON / intégration du checker sur signature)
- [x] rustc (natif, utilisé par cargo)

### Test (indispensables — propriétés et contrat)
- [x] cargo test
- [x] proptest (property-based — durcissement module critique, voir analyse)
- [ ] criterion (bench) — OPTIONNEL, réservé aux hot-paths mesurés (buffer DTN, parsing wire)

### Analyse / couverture (indispensables)
- [x] cargo llvm-cov (couverture diff maker/checker)
- [x] cargo-audit (sécurité deps — déjà actif, 139 crates scannées, clean)

### Crypto/audit (optionnels)
- [ ] cargo deny (OPTIONNEL : redondant avec cargo-audit au prototypage ; à activer en CI avant prod)
- [x] clippy pedantic (à la demande du checker, pas en gate quotidien)

## Choix de stack (rempli par l'acteur)

## CHOIX EFFECTUÉ (Stack du Dev Core Rust)

### Indispensables (workflow quotidien maker/checker)
1. **cargo build / cargo test + cargo clippy -- -D warnings + cargo fmt**
   - Baseline vérifié : `cargo build` OK, `cargo test` = 123 unit + 18 intégration (0 échec),
     `cargo clippy -- -D warnings` = 0 avertissement sur tout le workspace (core + 5 crates).
   - C'est le contrat quotidien : chaque itération maker sort sur `fmt` + `clippy -D` verts,
     le checker rejoue `cargo test` + `clippy -D warnings` pour valider. Zéro dette d'avertissement.
2. **rust-analyzer / rustc** — navigation + contrat de signature (présent, v1.96).
3. **proptest (property-based)** — à ajouter comme `dev-dependency` sur la cible n°1
   (parsing wire `protocol` + politique de buffer DTN). Les invariants (decode→encode,
   no-panic sur entrée tronquée, pad∘unpad = id) se prouvent par propriété, pas par cas isolé.
4. **cargo llvm-cov** — couverture par module pour cibler les gaps (voir analyse) et
   prouver au checker le Δ de couverture de chaque PR.
5. **cargo-audit** — sécurité dépendances : installé, scanne 139 crates, actuellement clean.
   Gate de CI, pas quotidien (charge réseau/index).

### Optionnels (à la demande / avant prod)
- **cargo-deny** — redondant avec cargo-audit au stade PROTOTYPE ; à activer en CI avant
  production (licences + yanked) sans dev le porter quotidiennement.
- **criterion (bench)** — réservé aux hot-paths mesurés (politique d'éviction du buffer DTN,
  parsing wire) quand un choix algorithmique doit être arbitré par le temps réel ; pas un gate
  quotidien au prototypage.
- **clippy pedantic** — invoqué ponctuellement par le checker sur des PR sensibles, pas en
  gate quotidien (bruit > valeur au prototypage).

## Module le plus critique à durcir en premier
**→ `src/protocol/mod.rs` (parsing wire + validation PoW/signature/gossip).**
Justification:
- C'est la **porte d'entrée de TOUT le trafic réseau** (DTN, event nostr, alerts, update,
  gossip) : chaque octet reçu d'un pair NON fiable passe par `from_wire_bytes` / `WireReader`.
  Un bug de parsing ou de validation = vecteur de corruption / attaque distante / désync
  du mesh.
- Il gate **avant** acceptation : validation PoW + signature Ed25519 + bornes (taille, TTL,
  geohash). C'est la frontière de confiance du réseau.
- C'est le terrain idéal du **proptest** : invariants constructibles sur parsing
  (round-trip `to_wire_bytes`→`from_wire_bytes`, no-panic sur buffers coupés/aléatoires,
  idempotence `unpad∘pad`, rejet de signatures/timestamps forgés).
- Les tests unitaires existants (25) couvrent les cas nominaux et quelques troncatures ;
  le durcissement ajoute la couche fuzz/property qui manque pour les entrées adverses.
Priorité 2 pour le prochain durcissement : `crates/dtn-router` (politique d'éviction du
buffer store-and-forward : priorités/ties/TTL — invariants par proptest + bench criterion).

## Rôle dans la boucle
- maker / checker / arbitre / analyse — (voir procédures)
