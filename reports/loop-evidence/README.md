# ONDE — Loop Evidence (preuves durables versionnées)

> Dossier VERSIONNÉ des preuves durables de la boucle d'agents ONDE (voir `.agentloop/` pour l'état
> courant et les preuves de travail ; ici sont rangées les preuves factuelles consolidées à conserver).

## Contexte
La boucle d'agents doit garder des **artefacts durables dans le repo** (pas seulement dans `.agentloop/`,
qui est hors versioning). Ce dossier est le lieu de stockage versionné des preuves d'audit / revue / validation.

## Inventaire
| Fichier | Quoi | Quand | Par qui | Verdict |
|---------|------|-------|---------|---------|
| `revue-crypto-l2-00-2026-08-20.md` | Revue sécurité (règle n°4) du diff orphelin L2-00, READ-ONLY | 2026-08-20 | security-auditor (session v1) | **GO** (signée) |
| `orphan-working-tree-l2-00.patch` | Patch factuel complet du working tree orphelin L2-00 avant engagement | 2026-08-20 | orchestrateur v1 (preuve d'étalage) | — (engendré `d87f8c9` + `07a4bd7`) |
| `cargo-test-workspace-2026-08-20.txt` | Sortie complète `cargo test --workspace` (`core/`, HEAD `00d03f3`) | 2026-08-20 | docs-writer (L2-03) | **163 passed / 0 failed** |

## Citations de commits (main)
- `28418e4` merge L2-01 — fix métrique livraison DTN (worktree l2-01)
- `d87f8c9` style(rustfmt) — passage audit core Rust (16 fichiers, +1096/-486)
- `07a4bd7` ci — durcissement pipeline (fmt+clippy -D warnings, npm/cargo audit hebdo, least-privilege)

## Synthèse L2-01 — bug métrique simulation (maker data-sim / checker code-reviewer)

**Bug** : `delivery_rate_percent` hors échelle (32 953 % observé au triage) —
`delivered` incrémenté par voisin **sans dédup** + re-forward en boucle ;
latence affichée 0,0 s.

**Fix** : livraison unique par message (dédup) + latence réelle —
commit `34356a3`, merge `28418e4`.

**Preuves (rejouées par le checker)** :
- pytest **7/7** verts sur le code corrigé ;
- anti-tautologie : **5/7 tests rouges** sur le code ancien ;
- même seed : delivery **87,29 %** / latence **5,965 s** (nouveau) vs **4287,65 %** / 0,0 s (ancien) ;
- propagation identique (`expired` / `dtn_hops` / `pow`) — le fix ne fausse pas le reste ;
- **verdict : CHECKER APPROVED** (6/6 critères, 3 réserves mineures non bloquantes).

> ⚠️ Preuves complètes (sorties pytest, rapport de checker) : `.agentloop/` —
> **non versionné**, à lire dans le clone principal :
> `~/onde-resilience-clone/.agentloop/evidence/` (artefacts) et
> `~/onde-resilience-clone/.agentloop/STATE.md` §5 (registre canonique des preuves).
> Seules les preuves consolidées ci-dessus sont durablement versionnées dans ce dossier.

---
Documenté le 2026-08-20 04:17:22 — orchestrateur v1 (commit baseline L2-00).
Mis à jour le 2026-08-20 — docs-writer (L2-03) : synthèse L2-01 + preuve cargo test 163 verts.
