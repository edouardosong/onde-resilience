# ONDE — Feuille de route v3.0.0

> Objectif : transformer le prototype (v0.2.x) en un **réseau de résilience
> citoyen utilisable en production** — hors-ligne, multi-transport,
> auto-réparateur, multi-plateforme. Cible : **Q3–Q4 2025**.
>
> Ce document est le **contraint directeur** de l'orchestration. Chaque
> tâche doit être **testable, isolée, et vérifiable** (tests unitaires +
> Playwright (open source) pour le frontend). Hermes orchestre, opencode implémente.

## Principes non négociables
1. **Honnêteté technique** : un module mock est étiqueté `MOCK` et n'est
   jamais présenté comme une sécurité réelle (cf. `ZkProof`, `llm-inference`).
2. **Aucune dépendance réseau par défaut** : tout doit fonctionner sans
   internet (DTN store-and-forward).
3. **Crypto réelle** : Ed25519, X25519, ChaCha20-Poly1305, HKDF. Pas de
   crypto "placeholder" en production (le stub `rust-core` a été supprimé).
4. **Zéro secret en clair** sauf documenté et assumé (cf. seed SQLite).
5. **Tout est testé** : `cargo test --workspace` green + clippy 0 warning
   avant tout merge.
6. **Playwright** (open source, MIT) valide le comportement UI réel (démarrage
   nœud, publication, flux, réputation) — pas seulement les tests unitaires.

## État actuel (baseline — mesurée le 2026-08-21, HEAD `7c888b3` = origin/main)
> Toutes les valeurs ci-dessous ont été **rejouées et mesurées le 2026-08-21**
> dans le worktree `t1-roadmap-truth` (base `origin/main` `7c888b3`) — aucun
> chiffre n'est repris d'un autre document sans rejeu.

- Moteur Rust : **163 tests verts, 0 échec** — `cargo test --workspace` dans
  `core/` (mesuré 2026-08-21, HEAD `7c888b3`) : onde_core 123 · integration_e2e
  18 · dtn-router 7 · llama-bind 5 · whisper-stt 4 · llm-inference 3 · zim-parser 3.
  Preuve versionnée (mesure du 2026-08-20, HEAD `00d03f3`) :
  `reports/loop-evidence/cargo-test-workspace-2026-08-20.txt`.
- `cargo clippy --workspace --all-targets` (dans `core/`) : **0 warning**
  (mesuré 2026-08-21).
- Simulation : **23 tests pytest verts, 0 échec** — `uv run pytest -q` dans
  `simulation/` (mesuré 2026-08-21) ; 11k nœuds validés, métriques de livraison
  corrigées (L2-01).
- E2E : **18 tests d'intégration core verts** (mesurés 2026-08-21, cf.
  `integration_e2e` ci-dessus) + **5 specs Playwright passées + 1 skip
  documenté** — `npx playwright test` dans `ui/web/` (mesuré 2026-08-21) ;
  skip : écran réputation WoT absent de l'UI web (baseline L2-06).
- Crypto, DTN, WoT, SQLite, sharding Geohash, tiers de rétention, protocole
  d'update APK (construit, câblé dans le gossip — Phases 1.1–1.4, tests e2e verts),
  bridge Tauri, shell Android.

Commits de référence (main) : `7c888b3` (HEAD = merge L2-14) · `28418e4` (merge L2-01 sim) ·
`d87f8c9` (L2-00 core) · `07a4bd7` (L2-00 CI).

---

## PHASE 0 — Consolidation de la baseline (Sprint 1)
> Objectif : figer le socle, corriger les écarts, tout publier.

| # | Tâche | Critère d'acceptation | E2E (Playwright) |
|---|-------|----------------------|-----------|
| 0.1 | Fix README (129→135) + commit propre — **FAIT** (2026-08-20 : README aligné sur le compte mesuré, 163 tests) | README = réalité | — |
| 0.2 | Publication GitHub de la passe "Audit #8–#14" | PR mergée, CI green | ✅ **FAIT** (2026-08-21 : dépôt public `github.com/edouardosong/onde-resilience`, main = 7c888b3) |
| 0.3 | Configurer Playwright (open source, sans clé API) | config + `npx playwright test` exécutable | ✅ **FAIT** (2026-08-20 : `ui/web/playwright.config.ts`, Playwright 1.62.1, 0 vuln.) |
| 0.4 | Baseline Playwright : 5 tests UI (démarrage, publish alerte, publish entraide, feed, réputation) | Suite durable green | ✅ **FAIT** (2026-08-20 : `ui/web/e2e/` 5 specs, 5 passed + 1 skip documenté [WoT UI absente], 4 runs verts consécutifs) |

---

## PHASE 1 (v1.0.0) — Réseau fonctionnel, démo de bout en bout
> Objectif : deux nœuds réels échangent un message signé, chiffré,
> propagé, et mis à jour — **sans internet**.

| # | Tâche | Critère d'acceptation | Statut (preuve) |
|---|-------|----------------------|-----------------|
| 1.1 | **Câblage protocole update** dans le gossip (types `UpdateAnnounce/Manifest/Chunk`) | Annonce → manifeste → chunks → APK vérifié entre 2 nœuds | ✅ **FAIT** — code `core/src/update/` ; e2e `test_update_flow_between_two_nodes` + `test_update_rejects_tampered_apk` **verts (mesurés 2026-08-21**, 18/18 `integration_e2e`) |
| 1.2 | **Propagation WoT** : message `Endorsement` routé dans le protocole | Un endossement propagé à tous les pairs (test e2e) | ✅ **FAIT** — code `core/src/reputation/` ; e2e `test_endorsement_propagation_three_nodes` **vert (mesuré 2026-08-21)** |
| 1.3 | **Câblage TrafficPadding** dans le transport (pad/unpad des messages réseau) | Taille observée = seau, contenu intact | ✅ **FAIT** — `TrafficPadding` câblé au wire gossip (`core/src/crypto/mod.rs`, `GossipProtocol`) ; e2e `test_traffic_padding_wire_two_nodes` **vert (mesuré 2026-08-21)** |
| 1.4 | **Câblage RotatingIdentity** : rotation 6h active + période de grâce | Rotation testée, anciens messages vérifiables, nouveaux rejetés | ✅ **FAIT** — `announce_identity_rotation` / `handle_incoming_rotation` (`core/src/node/mod.rs`) ; e2e `test_identity_rotation_two_nodes` + 4 tests de rejet à la réception (dont 3 anti-replay/anti-forgery) **verts (mesurés 2026-08-21)** |
| 1.5 | **Transport réel (Wi-Fi Aware)** — abstraction `MeshTransport` implémentée | 2 appareils réels échangent | ⛔ **BLOCKED** — 2 appareils physiques Wi-Fi Aware requis, **indisponibles dans cet environnement** (pas de matériel ; STATE §6, 2026-08-21) |
| 1.6 | **Transport BLE** (fallback faible débit) | Échange fonctionnel + chunking | ⛔ **BLOCKED** — 2 appareils physiques BLE requis, **indisponibles dans cet environnement** (pas de matériel ; STATE §6, 2026-08-21) |
| 1.7 | **Scénario de bout en bout** : alerte critique publiée → propagée → stockée (sharding) → restaurée au redémarrage | Test e2e complet green | ✅ **FAIT** — e2e `test_e2e_critical_alert_full_lifecycle` + `test_update_flow_between_two_nodes` **verts (mesurés 2026-08-21**, 18/18 `integration_e2e`) |

> Note (honnêteté) : l'historique git actuel (32 commits, plus ancien
> `eed8c78`) ne contient pas de commit dédié aux tâches 1.1–1.4 — la preuve
> FAIT repose sur le code présent dans l'arbre (`core/src/update/`,
> `core/src/reputation/`, `core/src/crypto/mod.rs`, `core/src/node/mod.rs`)
> et les tests e2e correspondants, tous rejoués verts le 2026-08-21.

**Livrable v1.0.0** : démo vidéo/écran de deux nœuds émettant et recevant
une alerte hors-ligne, avec mise à jour d'APK signée.

---

## SÉRIE L2 (durcissement + simulation) — voie critique
> Itération de durcissement et de fiabilité de la simulation (2026-08-20/21),
> menée entre Phase 0 et Phase 2. **13/13 items FAIT**, chacun mergé dans
> `main` avec verdict checker (STATE.md §5 — registre canonique des preuves).
> Numérotation : les numéros **02 et 07 n'existent pas** dans l'historique
> (`git log --oneline` des 32 commits — aucun commit ne les référence).

| ID | Item | Commit(s) | Statut |
|----|------|-----------|--------|
| L2-00 | Durcissement CI (fmt + clippy `-D warnings`, audit npm/cargo hebdo, least-privilege) + passage rustfmt core ; revue crypto du diff orphelin **GO** (preuve versionnée `reports/loop-evidence/revue-crypto-l2-00-2026-08-20.md`) | `07a4bd7` (CI) · `d87f8c9` (rustfmt) · preuves `5746365` | ✅ FAIT |
| L2-01 | Fix métrique livraison DTN — livraison unique par message (dédup) + latence réelle (checker APPROVED) | `34356a3` · merge `28418e4` | ✅ FAIT |
| L2-03 | README = vérité + ROADMAP aligné + preuves versionnées | `bfbb980` · `4ff7e33` · merge `c0ac9f9` | ✅ FAIT |
| L2-04 | `rencontre_opportunity` — bucketing spatial EXACT + dispatch | `4887f4b` · merge `6905364` | ✅ FAIT |
| L2-05 | Scaffolding Python simulation reproductible (`pyproject.toml` + `uv.lock` versionnés) | `4fe00cf` · merge `a298a0c` | ✅ FAIT |
| L2-06 | Baseline E2E Playwright — 5 specs UI + config, captures d'évidence (checker APPROVED) | `7a15ff4` · merge `f88c87b` · docs `d8a798a` | ✅ FAIT |
| L2-08 | `msg_id` unique par émission — séquenceur par expéditeur | `aa2c05d` · merge `1e6fa89` | ✅ FAIT |
| L2-09 | Émergence PoW documentée + constante `MAX_ATTEMPTS` | `ddf39df` · merge `3d5b233` | ✅ FAIT |
| L2-10 | Sémantique métriques par-COPIE / par-MESSAGE + 4 tests régression encounter (checker LOCAL APPROVED) | `bcf8fd3` · merge `8d8744a` | ✅ FAIT |
| L2-11 | `tx_id` unique par émission — séquenceur dans `ZKTransactionEngine` | `2c793b4` · merge `41ad6b2` | ✅ FAIT |
| L2-12 | `run_simulation(seed=42)` — déterminisme garanti par l'API | `a59ad4e` · merge `bf61efd` | ✅ FAIT |
| L2-13 | Isolation e2e — TempDir RAII (fin du flake `test_e2e_critical_alert`) | `46251d2` · merge `ec3d083` | ✅ FAIT |
| L2-14 | Bucketing multi-tiers rencontre — ADR-001 + perf + exactitude FP (checker LOCAL APPROVED) | `082adba` · merge `7c888b3` (= HEAD) | ✅ FAIT |

---

## PHASE 2 (v2.0.0) — Intelligence locale + durcissement
> Objectif : remplacer les mocks par du réel, rendre le nœud autonome.

| # | Tâche | Critère d'acceptation |
|---|-------|----------------------|
| 2.1 | **LLM local réel** : `llama-bind` → décodeur GGUF (llama.cpp), inférence sur-device | Question → réponse cohérente, RAM bornée |
| 2.2 | **STT réel** : `whisper-stt` → moteur Whisper.cpp | Audio → transcription (test sur corpus) |
| 2.3 | **ZIM/Wikipedia offline** : chargement ZIM réel, recherche + affichage | Consultation hors-ligne (Playwright) |
| 2.4 | **Cartes MBTiles offline** : rendu tuiles réelles | Affichage carte hors-ligne (Playwright) |
| 2.5 | **Mode batterie** : profils mobile/desktop/gateway adaptatifs (déjà présent) + vérif conso | Bench charge/décharge |
| 2.6 | **Compression + budget mémoire** : validation sur gros volume | Pas d'OOM à 100k messages |
| 2.7 | **Réputation anti-abus** : pénalités propagées, détection de spam | Attaque de spam contenue (test) |

**Livrable v2.0.0** : nœud autonome avec IA locale (questions santé/
premier secours), consultation offline, sans cloud.

---

## PHASE 3 (v3.0.0) — Production, échelle, auto-réparation
> Objectif : passer de "fonctionne" à "robuste à grande échelle,
> surveillé, et auto-réparateur".

| # | Tâche | Critère d'acceptation |
|---|-------|----------------------|
| 3.1 | **Audit de sécurité professionnel** + corrections | Rapport + 0 finding critique |
| 3.2 | **Fuzzing** : `cargo-fuzz` sur crypto, protocole, parsing | 0 crash exploitable (budget horaire) |
| 3.3 | **Simulation 11k nœuds** : validation stabilité + performance | Throughput/latence mesurés |
| 3.4 | **Auto-réparation** : détection de partition, re-sync, heal | Partition → reconvergence (test) |
| 3.5 | **Multi-plateforme** : Android (Play), iOS, Desktop (macOS/Win/Linux) | Builds signés |
| 3.6 | **Observabilité** : métriques, logs structurés, health endpoint | Dashboard de santé |
| 3.7 | **Mise à jour continue** : canal de release signé + rollback | MAJ de v2→v3 en production |
| 3.8 | **Documentation opérateur** : guide déploiement, runbook | Doc complète |
| 3.9 | **ZK transactions réelles** (optionnel) : vraie preuve SNARK ou retrait du claim | Pas de "ZK" mock présenté comme réel |

**Livrable v3.0.0** : production — réseau de résilience déployable,
audité, testé à grande échelle, auto-réparateur, multi-plateforme.

---

## Métriques de succès (KPI)
- **Fonctionnel** : 2+ nœuds réels, échange hors-ligne, MAJ signée.
- **Qualité** : 100% tests green, 0 clippy warning, 0 secret en clair,
  0 finding critique audit.
- **Performance** : 11k nœuds stables, RAM bornée, latence < seuil.
- **UX** : démo end-to-end fluide (Playwright 100% green).
- **Confiance** : crypto réelle, WoT décentralisé, audit indépendant.

## Rôles
- **Hermes** : orchestration, revue de code, tests, publication,
  arbitrage, Playwright, planification, mémoire de projet.
- **opencode** : implémentation de chaque tâche, tests unitaires,
  itérations locales.

## Règle de non-arrêt
> Tant que v3.0.0 n'est pas livrée, la boucle continue :
> **planifier → déléguer → vérifier → publier → itérer**.
