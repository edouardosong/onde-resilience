# ONDE — Loop Evidence (preuves durables versionnées)

> Dossier VERSIONNÉ des preuves durables de la boucle d'agents ONDE (voir `.agentloop/` pour l'état
> courant et les preuves de travail ; ici sont rangées les preuves factuelles consolidées à conserver).

## Contexte
La boucle d'agents doit garder des **artefacts durables dans le repo** (pas seulement dans `.agentloop/`,
qui est hors versioning). Ce dossier est le lieu de stockage versionné des preuves d'audit / revue / validation.

## Inventaire actuel
| Fichier | Type | Preuve de |
|---------|------|-----------|
| `revue-crypto-l2-00-2026-08-20.md` | Revue sécurité (verdict GO signé) | Commit baseline L2-00 : diff orphelin 17 fichiers revu en READ-ONLY, règle n°4 satisfaite |
| `orphan-working-tree-l2-00.patch` | Patch factuel complet | État exact du working tree orphelin L2-00 avant engagement (convient à d87f8c9 + 07a4bd7) |

## Citations de commits (baseline L2-00, main)
- `d87f8c9` style(rustfmt) — passage audit core Rust (16 fichiers, +1096/-486)
- `07a4bd7` ci — durcissement pipeline (fmt+clippy -D warnings, npm/cargo audit hebdo, least-privilege)

## À compléter (par l'orchestrateur v2, via L2-03)
- Preuves L2-01 (bug métrique sim) : pytest avant/après + verdict checker — commit `34356a3` / merge `28418e4`.

---
Documenté le 2026-08-20 04:17:22 — orchestrateur v1 (commit baseline L2-00).
