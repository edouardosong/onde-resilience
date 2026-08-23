# AGENT : Auditeur Sécurité / SecOps

## Mission
Auditer la surface d'attaque (prototype non durci !) : auth/crypto, entrées non fiables, dépendances, secrets.

## Rôle dans la boucle
**CHECKER** (maker/checker : le security-auditor ne fait JAMAIS de merge). Code crypto/auth/finance → audit sécurité OBLIGATOIRE avant gate (MANIFEST §3).
Statut v2 confirmé, 2026-08-20.

---

## CHOIX EFFECTUÉ (déposé v2 — 2026-08-20, validé par exécution réelle sur le clone)

> L'acteur a exécuté chaque outil retenu sur `/home/linux/onde-resilience-clone` (pas de choix théorique).
> Écosystème majoritaire = **Rust** (`core/` ≈ 11 000 lignes, TOUTE la surface de sécurité réelle).
> `android/` = stub (1 fichier Java, 0 Kotlin `.kt`), `onde/`+`simulation/` = 2 scripts Python (~800 lignes).
> → la priorité SAST/SCA est le code Rust du core.

### SAST
- [x] **cargo clippy --pedantic**  — **INDISPENSABLE** (seul SAST réellement applicable : la surface = Rust core ; clippy est le linter officiel, installé, exécuté, passe à 0 warning).
- [ ] semgrep — non installé, install lourd (engine Python/Ocaml) ; **recommandé +tard en CI** pour des patterns de règles cross-couches (crypto misuse, taille de buffer), une fois le reste stabilisé.
- [ ] detekt/Android Lint — **non applicable** : `android/` est un stub (MainActivity.java, pas de `.kt`, apk non signé). Aucune valeur à auditer aujourd'hui.
- [ ] bandit (Python) — non applicable (surface Python = 2 scripts sim non restreints) ; suivi possible mais priorité basse.

### SCA (dépendances)
- [x] **cargo audit**  — **INDISPENSABLE** (installé, exécuté : **0 vulnérabilité sur 139 dépendances de Cargo.lock**).
- [ ] **OSV-Scanner** — recommandé **en CI** (couvre Python/JS/Gradle au-delà de Rust) ; binaire Go non dispo offline ici, pip n'expose pas de CLI → différé.
- [ ] **Dependabot** — recommandé **sur GitHub** (automatise l'alerte de vulnérabilité + PR de bump), ne tourne pas localement.
- [ ] cargo deny — non installé ; localStorage des licensing/bans adressable plus tard, cargo audit suffit en SCA v1.

### Secret scanning
- [x] **detect-secrets**  — **INDISPENSABLE** (installé via `uv tool install detect-secrets`, exécuté) : scan `--all-files` → **0 vrai secret** (les 3 hits sont des faux positifs : base32 alphabet géohash `storage/mod.rs:210`, pubkey placeholder de test `integration_e2e.rs:146`, meta `core/target/`).
- [ ] gitleaks / trufflehog — outils Go non installés (pas de `go`) ; detekt-secrets + audit manuel git-history couvre le besoin v1. Envisager gitleaks en CI pour le scan d'historique git.

### Runtime / fuzzing
- [ ] **cargo-fuzz** — **recommandé MAIS différé** : nécessite toolchain nightly, et le prototype n'est pas prêt (des panics connues par entrée non fiable casseraient le fuzzer avant de trouver du neuf). Débloquer dès que parsing durci + tests de bounds présents.
- [ ] AFL++ — non applicable (nécessite instrumentation LLVM/AFL, utile qu'après durcissement parsing).

### SYNTHÈSE — indisponibles à exécuter dès maintenant
1. `cargo clippy --pedantic` (✔ exécuté, 0 warning)
2. `cargo audit` (✔ exécuté, 0 vuln sur 139 deps)
3. `detect-secrets` (✔ exécuté, 0 vrai secret)

Complément CI (recommandés, non exécutés localement) : OSV-Scanner, Dependabot, gitleaks, semgrep.
Différé (après durcissement parsing) : cargo-fuzz, AFL++.

---

## SURFACE D'ATTAQUE — 5 ZONES LES PLUS À RISQUE
(prototype mesh offline, crypto custom — classées par criticité)

1. **Parsing de protocole / entrées non fiables — `core/src/protocol/mod.rs` (1 369 lignes)**
   Le cœur gossip reçoit des événements d'origine non fiable (n'importe quel pair du mesh sans internet). 15 `unwrap` + 7 `expect` sur données entrantes → **panique = crash de nœud**. Des caps existent (MAX_ALERT_SIZE=280, MAX_KNOWN_EVENTS=10k, MAX_PENDING_BROADCASTS=1k, MAX_DELIVERED_PER_PEER=2k) mais plusieurs sites de parse restent non bornés → déni de service / amplification.

2. **DoS par épuisement mémoire — `core/crates/dtn-router` (buffers store-and-forward)**
   `DtnMessage.payload: Vec<u8>` et `delivered_to: Vec<String>` sont désérialisés directement depuis du réseau non fiable **sans limite de taille**. Pire : `store()` considère `max_buffer == 0` comme « jamais plein » → un pair malveillant peut inonder le buffer et faire saturer la RAM d'un mobile mesh (capacité limitée). C'est le constat untrusted-input le plus tranché.

3. **Crypto identité — couplage Ed25519/X25519 — `core/src/crypto/mod.rs`**
   X25519 (chiffrement) dérivé DÉTERMINISTIQUEMENT de la seed Ed25519 via HKDF (`derive_x25519`). Une seule compromission de seed = perte simultanée **signature ET chiffrement**. Délibéré (restauration mono-secret) mais = point de défaillance unique. L'enveloppe ChaCha20-Poly1305 (ECDH éphémère + AEAD, sender_pubkey lié au tag) est bien conçue ; le risque porte sur la dérivation couplée, pas le chiffrement en lui-même.

4. **ZK Transaction / TxPool — `core/src/crypto/mod.rs` (MOCK explicitement non-SNARK)**
   `ZkProof` n'est PAS une vraie preuve à divulgation nulle (Groth16/Plonk/STARK) : `verify()` ne contrôle que l'intégrité structurelle (SHA-256 déterministe). Code commenté : « ne jamais utiliser pour de la valeur réelle ». **Risque = câblage accidentel sur de la vraie valeur** (bridge/feuille) ou confiance erronée en son intégrité — à garder étiqueté MOCK et hors de tout chemin production.

5. **Chaîne de distribution APK — `core/src/update/mod.rs` + `crypto::verify_apk_signature`**
   La chaîne racine épinglée (magic ONDEAPK1, signature Ed25519 + SHA-256 de l'APK, vérif de bout en bout) est **bien conçue**. Mais : les métadonnées de transfert (taille APK / taille de chunk) sont **NON signées** (uniquement liées au hash), et l'assemblage/stockage des chunks + le transport du fichier entier dans le mesh = surface d'amplification bande-passante / DoS. Le vérificateur gère les limites (MAX_APK_SIZE, MAX_CHUNKS) mais à re-lire.

---

## CONSTATS (exécutions réelles sur le clone, 2026-08-20)

- **SCA (cargo audit, core/)** : `Cargo.lock` 139 crates → **0 vulnérabilité connue**. ✔
- **SAST (clippy pedantic, -D warnings -W clippy::pedantic)** : workspace + all-targets → **0 warning clippy**. ✔
- **Secret scanning (detect-secrets scan --all-files)** : **0 vrai secret**. 3 hits = faux positifs (base32 alphabet `storage/mod.rs:210`, pubkey deadbeef de test `integration_e2e.rs:146`, meta build `core/target/`). ✔
- **Panics sur entrée non fiable** : protocol 15 unwrap + 7 expect ; storage 30 unwrap + 10 expect (sur chemin de persistance SQLite). Zone 1 principale.
- **Identité crypto** : couplage Ed25519→X25519 assumé, point de défaillance unique (zone 3).
- **ZK** : MOCK non-SNARK, aucune valeur réelle (zone 4).
- **Android** : stub non signé, pas de `.kt` — hors périmètre critique pour l'instant.

---

## Traitement recommandé avant durcissement (ordre)
1. **Borner le DTN buffer** (payload cap + vrai max_buffer, pas `max_buffer==0`=infini).
2. **Remplacer les unwrap/expect du parsing protocole** par des erreurs (Option/Result) + caps de taille explicites.
3. **Ré-examiner la dérivation couplée Ed25519/X25519** (documenter + envisager split de domaine si la restauration mono-secret n'est pas un besoin dur).
4. **Garder ZK strictement MOCK** ; ajouter un garde-fou compile-time/CI empêchant tout usage production.
5. **Signer/valider les métadonnées de transfert APK** ou lier explicitement taille/chunk au hash signé.
6. **Débloquer cargo-fuzz sur les parsers** (protocole, dtn, ZIM) une fois 1-2 faits.

## Commande de vérif de non-régression (à rejouer au gate)
```bash
cd core && cargo audit && cargo clippy --workspace --all-targets -- -D warnings -W clippy::pedantic
detect-secrets scan --all-files   # depuis racine clone
```
