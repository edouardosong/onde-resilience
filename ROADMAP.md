# ONDE — Feuille de route v3.0.0

> Objectif : transformer le prototype (v0.2.x) en un **réseau de résilience
> citoyen utilisable en production** — hors-ligne, multi-transport,
> auto-réparateur, multi-plateforme. Cible : **Q3–Q4 2025**.
>
> Ce document est le **contraint directeur** de l'orchestration. Chaque
> tâche doit être **testable, isolée, et vérifiable** (tests unitaires +
> TestSprite pour le frontend). Hermes orchestre, opencode implémente.

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
6. **TestSprite** valide le comportement UI réel (démarrage nœud, publication,
   flux, réputation) — pas seulement les tests unitaires.

## État actuel (baseline, vérifié Hermes)
- Moteur Rust : **163 tests verts, 0 échec** — mesurés le 2026-08-20
  (`cargo test --workspace` dans `core/`, HEAD `00d03f3`), clippy 0 warning,
  Tauri build OK. Preuve versionnée :
  `reports/loop-evidence/cargo-test-workspace-2026-08-20.txt`.
- Crypto, DTN, WoT, SQLite, sharding Geohash, tiers de rétention, protocole
  d'update APK (construit, câblé dans le gossip — Phases 1.1–1.4, tests e2e verts),
  bridge Tauri, shell Android.
- Simulation 11k nœuds (v0.2.5) validée — métriques de livraison corrigées (L2-01).

Commits de référence (main) : `28418e4` (merge L2-01 sim) · `d87f8c9` (L2-00 core) ·
`07a4bd7` (L2-00 CI).

---

## PHASE 0 — Consolidation de la baseline (Sprint 1)
> Objectif : figer le socle, corriger les écarts, tout publier.

| # | Tâche | Critère d'acceptation | TestSprite |
|---|-------|----------------------|-----------|
| 0.1 | Fix README (129→135) + commit propre — **FAIT** (2026-08-20 : README aligné sur le compte mesuré, 163 tests) | README = réalité | — |
| 0.2 | Publication GitHub de la passe "Audit #8–#14" | PR mergée, CI green | — |
| 0.3 | Configurer TestSprite (clé + skill agent + project) | `testsprite doctor` green | ✅ |
| 0.4 | Baseline TestSprite : 5 tests UI (démarrage, publish alerte, publish entraide, feed, réputation) | Suite durable green | ✅ |

---

## PHASE 1 (v1.0.0) — Réseau fonctionnel, démo de bout en bout
> Objectif : deux nœuds réels échangent un message signé, chiffré,
> propagé, et mis à jour — **sans internet**.

| # | Tâche | Critère d'acceptation |
|---|-------|----------------------|
| 1.1 | **Câblage protocole update** dans le gossip (types `UpdateAnnounce/Manifest/Chunk`) | Annonce → manifeste → chunks → APK vérifié entre 2 nœuds |
| 1.2 | **Propagation WoT** : message `Endorsement` routé dans le protocole | Un endossement propagé à tous les pairs (test e2e) |
| 1.3 | **Câblage TrafficPadding** dans le transport (pad/unpad des messages réseau) | Taille observée = seau, contenu intact |
| 1.4 | **Câblage RotatingIdentity** : rotation 6h active + période de grâce | Rotation testée, anciens messages vérifiables, nouveaux rejetés |
| 1.5 | **Transport réel (Wi-Fi Aware)** — abstraction `MeshTransport` implémentée | 2 appareils réels échangent |
| 1.6 | **Transport BLE** (fallback faible débit) | Échange fonctionnel + chunking |
| 1.7 | **Scénario de bout en bout** : alerte critique publiée → propagée → stockée (sharding) → restaurée au redémarrage | Test e2e complet green |

**Livrable v1.0.0** : démo vidéo/écran de deux nœuds émettant et recevant
une alerte hors-ligne, avec mise à jour d'APK signée.

---

## PHASE 2 (v2.0.0) — Intelligence locale + durcissement
> Objectif : remplacer les mocks par du réel, rendre le nœud autonome.

| # | Tâche | Critère d'acceptation |
|---|-------|----------------------|
| 2.1 | **LLM local réel** : `llama-bind` → décodeur GGUF (llama.cpp), inférence sur-device | Question → réponse cohérente, RAM bornée |
| 2.2 | **STT réel** : `whisper-stt` → moteur Whisper.cpp | Audio → transcription (test sur corpus) |
| 2.3 | **ZIM/Wikipedia offline** : chargement ZIM réel, recherche + affichage | Consultation hors-ligne (TestSprite) |
| 2.4 | **Cartes MBTiles offline** : rendu tuiles réelles | Affichage carte hors-ligne (TestSprite) |
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
- **UX** : démo end-to-end fluide (TestSprite 100% green).
- **Confiance** : crypto réelle, WoT décentralisé, audit indépendant.

## Rôles
- **Hermes** : orchestration, revue de code, tests, publication,
  arbitrage, TestSprite, planification, mémoire de projet.
- **opencode** : implémentation de chaque tâche, tests unitaires,
  itérations locales.

## Règle de non-arrêt
> Tant que v3.0.0 n'est pas livrée, la boucle continue :
> **planifier → déléguer → vérifier → publier → itérer**.
