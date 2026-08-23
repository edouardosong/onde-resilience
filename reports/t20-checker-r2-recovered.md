# T20 — verdict checker r2 RÉCUPÉRÉ (evidence complémentaire)

> Provenance : session code-reviewer-t20-r2 (sub-194a6bce) terminée sans reply final —
> contenu reconstitué le 2026-08-23 via observation de transcript (agent_observe) par l'orchestrateur.
> Statut session : revue statique 100 % terminée ; rejeu dynamique interrompu par pause du heartbeat
> (la mesure décisive existe déjà via T20-r3 en mode release, voir ROADMAP/§5).

## Revue statique (COMPLÈTE, toutes vérifications vertes)
1. Diff exactement 1 fichier : core/tests/mem_budget_stress.rs (+333) ; Cargo.lock INTACT ; zéro fichier métier.
2. Chemin RÉEL sans mock confirmé ligne à ligne : MeshEvent signé → to/from_wire_bytes →
   receive_peer_event (signature/dédup/SpamGuard) → handle_incoming_alert (validate_with_reputation +
   gossip bornée + stockage Deflate + SQLite WAL).
3. Déterminisme : seul std::time::Instant::now() à L164, usage affichage uniquement — aucune assertion
   temporelle ; pas de sleep/rand/SystemTime dans la logique de test.
4. Assertions contraignantes présentes (seuil RSS asserté, compteurs stored/restored exacts, ratio vérifié).
5. Test compression intermédiaire PASS pendant la session : corpus compressible brut=2 176 000 B →
   stocké=256 000 B (ratio 0,12).

## Rejeu dynamique (PARTIEL — interrompu)
- ONDE_STRESS=1 lancé par le checker : stress_ingest_memory_bounded en exécution normale (>60 s/37 min)
  au moment de la pause. Aucun échec constaté avant interruption.

## Croisement avec les autres sources
| Source | Mode | Pic RSS 100k | Statut |
|---|---|---|---|
| Maker (c0f0cd8) | debug | 54 MiB ingestion / 103 MiB restauration | exit 0 |
| T20-r3 (siège actif) | release | 51 MiB | exit 0, roadmap VALIDÉ |
| Checker r2 (présent document) | debug | (rejeu interrompu, statique complète) | statique ✓ |

**Conclusion consolidée : T20 VERIFIÉE** — seuil 256 MiB respecté avec marge ×2,4 en release ;
aucune correction métier nécessaire (mémoire bornée par design via caps existants).


---

# ADDENDUM (2026-08-23 ~21:15) — VERDICT FINAL LIVRÉ par code-reviewer-t20-r2

La session a finalement livré son verdict complet via agent_message (après la récupération
ci-dessus). Résumé des apports NOUVEAUX par rapport à la récupération :

## Preuve négative OUTILLÉE par mutations manuelles (les deux FAIL comme attendu, revert vérifié)
- (a) seuil RSS 256→5 MiB → FAIL exit 101 « pic RSS 9 MiB > seuil » ✓
- (b) restored n→n−1 → FAIL exit 101 (500 vs 499) ✓

## Rejeu dynamique COMPLET (debug, 38,5 min) — identique aux claims maker au chiffre près
stored=100000 / rejected=0 / restored=100000 ; pic RSS 54 MiB (ingestion) / 103 MiB (restauration) ;
compression 23,4 Mo → 18,7 Mo (ratio 0.80) ; SQLite 37,7 Mo.

## Gates propres du checker : fmt 0 · clippy -D warnings 0 · gitleaks 0 (138 commits) · test --workspace 0 (~47 s CI-mode)

## Table d'assertions documentée fichier:ligne (L213-L332) — valeurs clés ASSERTÉES (seuils/comptes),
mesures machine en println informatif : bon design stress test.

## Convergence
T20 mergée en main `290ebb8` (19:41) sur preuve r2-récupérée + re-run release `d8dcca8` exit 0
(51/100 MiB < 256, ratio 0.80). Verdict r2 = confirmation indépendante POST-merge, CONVERGENTE → APPROVED.

## Findings non bloquants
F1 LOW : ratio exact 0.80 = println (assertion structurelle compressed<raw ; test dédié <raw/2).
F2 INFO : assert RSS skip non-Linux (VmHWM), documenté — CI ubuntu l'asserte.
F3 INFO : assert pow_difficulty tautologique vs new_signed hardcodé — documente l'intention.

## Incident inter-lanes consigné (à arbitrage humain)
Run stress #1 (19:14) tué par cleanup post-merge de l'autre orchestrateur ; run #2a (19:54) tué à ~20:26
par son triage « zombie #3 » ; run #2b (20:26→21:10) = run vert ci-dessus. Coût : ~1 h de calcul perdue.
