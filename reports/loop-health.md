# ONDE — Santé de la boucle (reports/loop-health.md)

> Mis à jour à chaque MÉMORISER (règle onde-loop étape 9, décision 2026-08-23).
> STATE.md garde les compteurs DÉCISOIRS (échecs par module) ; ce fichier trace les métriques analytiques.
> Données antérieures au 2026-08-23 : reconstituées depuis STATE.md §5/§7 — précision limitée, signalée n/d quand non mesurable.

## Snapshot 2026-08-23 (seed initial)

| Métrique | Valeur | Note |
|---|---|---|
| Tâches terminées | 17 depuis reconstruction spine (08-21) + 15 L2 pré-spine | T1,T3,T5,T7,T8,T9,T10,T11,T12,T13,T14,T15,T16,T17,T18,T19,T21 |
| Tâches bloquées actives | 2 | §6 : Wi-Fi Aware/BLE (hardware), T2 ADR-001 (décision humaine) |
| Échecs par module (cumul) | zim-parser 1 · mbtiles-parser 1 · autres 0 | cf. STATE.md §4 (décisoir) |
| Durée moyenne par tâche | ≈ 1 jour calendaire (17 tâches / ~2,5 j avec 2 lanes) | mesure horaire précise n/d avant 08-23 |
| Coût token estimé | lane Qwen local ≈ 0 € (LM Studio) ; lane ox-alpha = compteur provider, historique n/d | suivi futur : estimation grossière par tâche |
| Taux de flaky | 1 événement (T19 e2e, 1,75 %/run) → corrigé côté TEST uniquement, 0 récidive | quarantaine appliquée |
| Mutation score | T21 health/metrics : 37/58 caught (63,8 % brut) ; misses triés : ~11 pertinents (caps/busy) → cible T23 ≈ 100 % pertinent caught | passes archivées reports/t21-mutants, reports/t23-mutants |
| Drifts lockfile détectés en gate | 0 | règle --locked/npm ci active depuis 08-23 |
| Secrets détectés | 0 | gitleaks verts, dont scan full-history 70 commits (T21) |
| Gates rejetées (CHANGES_REQUESTED) | 5 (T1, T3, T11, T13, T17) — toutes → fixes → APPROVED ; 0 REJECT définitif | cycle preuve-négative efficace |
| Builds cassés post-merge | 0 régression code ; 1 panne CI workflows (SHAs d'actions inventés, externe au code, réparée T12-T16) | |

## Journal
- 2026-08-23 : seed initial + formalisation du suivi (propositions utilisateur 9.1-9.4 adoptées).
