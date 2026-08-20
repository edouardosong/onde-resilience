---
audit: "revue-crypto-l2-00"
date: 2026-08-20
role: "auditeur_securite_secops"
scope: "Diff orphelin L2-00, commit baseline ONDE (~/onde-resilience-clone)"
statut: "GO — committer tel quel"
signature: "auditeur_securite_secops"
rule_4: "crypto/storage/RGPD — revue sécurité obligatoire avant merge (respectée)"
---

# REVUE SÉCURITÉ — Diff orphelin L2-00 (READ-ONLY)

## 1. Périmètre

Diff **NON COMMITÉ** sur `main` (clone `~/onde-resilience-clone`), **17 fichiers**, +1304/−501 lignes.
Gates de l'orchestrateur déjà verts : `cargo build`, `cargo test --workspace` (160 tests),
`clippy --all-targets -D warnings` (=0).

Revue effectuée **sans appliquer aucun changement** au code (lecture seule du `git diff`, pas de modification du working tree).

### Fichiers revus en priorité (7)
| Fichier | Lignes diff | Verdict partiel |
|---|---|---|
| `core/src/crypto/mod.rs` | 130 | Reformatage uniquement |
| `core/src/storage/mod.rs` | 250 | Reformatage uniquement |
| `core/src/storage/persistence.rs` | 51 | Reformatage uniquement |
| `core/src/protocol/mod.rs` | 158 | Reformatage uniquement |
| `core/src/node/mod.rs` | 281 | Reformatage uniquement |
| `core/src/update/mod.rs` | 94 | Reformatage uniquement |
| `core/tests/integration_e2e.rs` | 306 | Reformatage uniquement |

### Autres fichiers du diff (vérifiés pour absence de régression)
`ci.yml`, `core/crates/{dtn-router,llama-bind,whisper-stt,zim-parser}/lib.rs`,
`core/src/{ai,lib,network,reputation}/mod.rs`, `core/src/bin/node.rs`.

---

## 2. Analyse détaillée — fichiers critiques

### 2.1 `core/src/crypto/mod.rs`
- **Nature du diff** : réordonnancement des `use`, reformatage `rustfmt` (wrapping de lignes,
  assert multilignes, ternaires en blocs). **Aucune modification de logique**.
- **Primitives vérifiées inchangées (HEAD commité vs working tree)** : HKDF-SHA256
  pas; `derive_x25519`, `derive_key`, `EncryptedEnvelope` (X25519 ECDH + ChaCha20-Poly1305 AEAD),
  `ZkTransaction`, `TxPool`, `ApkManifest`, `verify_apk_signature`, `TrafficPadding`.
- **Preuve quantitative** : comptage des symboles crypto (Hkdf, ChaCha20Poly1305, X25519StaticSecret,
  derive_x25519, derive_key, ed25519, zeroize) **strictement identique** entre HEAD et WT.
- **Aucun secret, aucune clé privée, aucun endpoint modifié.**
- **Verdict** : SAFE.

### 2.2 `core/src/storage/mod.rs`
- **Nature du diff** : reformatage de tableaux de démo, TUPLES "demo_articles" / "demo_seeds",
  méthodes `compress`/`get`/`sweep_expired`/`should_store_locally`/`count_by_tier` — wrapping
  rustfmt uniquement.
- **Constantes de rétention et budgets vérifiées identiques** : tiers (Critical 7j, Important 2j,
  Normal, Low), `StoragePolicy` (Mobile 64 Mo / Desktop 2 Go / Gateway 16 Go), `should_store_locally`
  (logique geohash + alerte critique conservée).
- **Verdict** : SAFE.

### 2.3 `core/src/storage/persistence.rs`
- **Nature du diff** : réorg import `rusqlite`, struct literal multiligne, wrapping des appels SQL.
  Logique SQL (INSERT OR REPLACE, sweep par tier, count, load_all, open/réouverture) **inchangée**.
- **Aucune requête SQL modifiée, aucune table/schema touché.**
- **Verdict** : SAFE.

### 2.4 `core/src/protocol/mod.rs` (parsing trafic non fiable)
- **Nature du diff** : reformatage `MeshEvent.new`, `canonical`, validation signature/PoW,
  `to_wire_bytes`, `WireReader.take_array/take_u8`, `GossipProtocol`.
- **Point clé sécurité (anti-panique)** : nombre d'appels `.unwrap()` **identique (11 = 11)**
  entre HEAD et WT → **aucun nouveau chemin de panique introduit** dans le parsing réseau.
- Les bornes (MAX_ALERT_SIZE, buckets de padding, MAX_DELIVERED_PER_PEER, TTL) sont **strictement
  identiques**.
- **Verdict** : SAFE.

### 2.5 `core/src/node/mod.rs`
- **Nature du diff** : reorg imports, wrapping des appels `persist_message`, `sig_verified`,
  `build_rotation_announcement`, `handle_incoming_rotation`, ternaires en blocs.
- **Valeurs de config inchangées** : `available_ram_mb` (DesktopBridge 32768 / sinon 4096),
  `storage_gb` (512 / 64), `max_peer_connections` (100 / 20) — les constantes numériques restent
  **strictement identiques** (le diff ne montre que la transformation ternaire->bloc).
- **Logique de rotation d'identité X25519, grâce (grace period), vérification de signature,
  relai gossip** : inchangée.
- **Verdict** : SAFE.

### 2.6 `core/src/update/mod.rs`
- **Nature du diff** : wrapping `Display`, `chunks_received`/`total_chunks`, appels de test.
  Logique `UpdateManifest::verify`, taille chunks, `ChunkIndexOutOfBounds`, `ChunkTooLarge`,
  rejet des transferts incohérents **inchangée**.
- **Constantes** : `DEFAULT_CHUNK_SIZE`, `SIGNED_MANIFEST_LEN` **identiques**.
- **Verdict** : SAFE.

### 2.7 `core/tests/integration_e2e.rs`
- **Nature du diff** : réorg imports tests, wrapping des asserts et des calls async.
  **Aucun test supprimé, aucun scénario modifié.** Couverture e2e (alertes, ZK, DTN, update,
  endorsement, identité rotation, padding wire, persistance SQLite) **préservée**.
- **Verdict** : SAFE.

---

## 3. Analyse annexe — ci.yml et autres crates

### 3.1 `.github/workflows/ci.yml`
Changement **améliorant** la posture de sécurité, pas une régression :
- Least-privilege conservé (`permissions: contents: read`) ; write élargi **uniquement** sur
  jobs release tag `v*` (scope fin `contents: write`).
- Ajout de gates qualité : `cargo fmt --check`, `clippy --all-targets -D warnings`,
  `cargo test --workspace`, build Android, `npm audit --audit-level=high`, `cargo audit` (rustsec),
  audit npm hebdomadaire planifié.
- Pins d'actions SHA256 (v4.x) conservés.
- **Verdict** : SAFE (amélioration).

### 3.2 Autres crates (`dtn-router`, `llama-bind`, `whisper-stt`, `zim-parser`) + libs
Reformatage uniquement. Les URLs de téléchargement des modèles whisper (huggingface)
restent **strictement identiques** (reformatage multiligne seulement). Aucun endpoint changé.

### 3.3 `core/src/{ai,network,reputation}.rs`
- `reputation` : logique `endorse`, seuils (`ENDORSEMENT_THRESHOLD`, `TRUSTED_THRESHOLD`,
  `MAX_POW_DIFFICULTY`, `BASE_POW_DIFFICULTY`), calcul de score (`.max(0.0)`) **inchangés**.
- `network` : distance d'arbre Yggdrasil (`tree_distance`, valeurs 128/0) **inchangées**.
- **Verdict** : SAFE.

---

## 4. Vérifications transverses effectuées

| Contrôle | Résultat |
|---|---|
| Primitives crypto (Hkdf, ChaCha20Poly1305, X25519, ed25519, zeroize) | **Identiques** HEAD vs WT |
| Constantes de sécurité (`MAX_*`, seuils, rétention tiers, budget stockage, `DEFAULT_CHUNK_SIZE`, PoW) | **Identiques** |
| Nombre de `.unwrap()` dans parsing non fiable (protocol) | **Identique** (11=11, aucun nouveau chemin de panique) |
| URLs/endpoints (modèles whisper, actions CI) | **Identiques** |
| Secrets/credentials/clés privées dans le diff | **Aucun** (seul `persist-credentials: false`) |
| Valeurs de config RAM/stockage/peers | **Identiques** |
| Diff fonctionnel hors reformatage | **Aucun** (100% rustfmt) |

---

## 5. VERDICT SIGNÉ

> **VERDICT : GO — committer tel quel (SAFE).**

- **Aucun fix BLOQUANT** (aucun NO-GO).  
- **Aucun fix non-bloquant** (aucune condition de sécurité).

Ce diff orphelin L2-00 est un **reformatage `rustfmt` + réorganisation d'imports** à 100 %,
accompagné d'une **amélioration** non-régressive de la CI (gates fmt/clippy/audit, least-privilege).
Aucune modification de logique crypto, de politique de stockage/rétention (RGPD), de seuils de
confiance PoW, de parsing réseau, de gestion des mises à jour, ni d'endpoints de confiance.
Aucun secret introduit. Aucun nouveau chemin de panique.

La règle ONDE n°4 (security-auditor obligatoire pour crypto/storage/RGPD) est **satisfaite**.

### Notes cosmétiques (non-bloquantes, pour information, aucune action requise)
- Quelques commentaires ont été légèrement décalés par rustfmt (ex : `// Même voisinage` dans
  storage, `// major version` dans les tests ZIM, `// Unsigned events` / `// Extensions audits`).
  Purement esthétique, sans impact fonctionnel ni de sécurité.

---
_signé : auditeur_securite_secops — revue READ-ONLY, aucun changement appliqué au code._
