# ONDE — AUTOMATIONS & DÉCLENCHEURS (v2)

Registre des déclencheurs qui font découvrir et trier le travail **sans humain**.

## Déclencheurs actifs
1. **Triage périodique** (heartbeat prime-agent, label `onde-triage`, intervalle 30 min, delivery_mode=follow_up)
   - Instruction : lire `.agentloop/STATE.md` (§0 protocole de reprise), vérifier dépôt (git status/log),
     exécuter UNE itération complète (ISOLER→MAKER→CHECKER→GATE→MERGE→MÉMORISER) sur la tâche la plus prioritaire,
     puis mettre à jour STATE.md. Rien d'actionnable → auto-archivage (pas d'activité artificielle).
   - Gestion : `await rlm_heartbeat.list()` / `.update(id, status="pause")` / `.delete(id)`.
2. **Goal mode** (run-until-done) — pour un objectif borné (ex. « Phase 0 terminée ») :
   `await goal.create("<objectif + condition de fin vérifiable>", token_budget=...)`
   - L'orchestrateur itère jusqu'à ce que la condition soit VÉRIFIÉE par un CHECKER (pas par le maker).
   - `await goal.complete()` uniquement quand la condition tient + preuve dans STATE.md §5.
3. **Gate de qualité** (à chaque modification) : tests du module + lint (clippy/ruff/Biome) verts — porte avant tout « terminé ».
4. **Revue de PR** : tout diff mergeable passe par un CHECKER indépendant (code-reviewer minimum ; + security-auditor si crypto/auth).

## Mode d'emploi (orchestrateur)
- Pause de la boucle : `await rlm_heartbeat.update("<id>", status="pause")`
- Reprise : `await rlm_heartbeat.update("<id>", status="resume")`
- Itération manuelle immédiate : « continue la boucle ONDE » (reprise via STATE.md §0)
- N'activer le heartbeat QUE si le dépôt a du travail (sinon il consomme sans produire — règle d'arrêt n°7).
