# PROCÉDURE : Arbitre — trancher les divergences maker/checker
Usage : quand maker et checker ne sont pas d'accord, ou à chaque décision de fusion.

1. Lire le diff, le verdict du checker, la réponse du maker.
2. Appliquer la règle de la boucle : en cas de doute sur auth/paiement/RGPD/prod → escalade humain.
3. Décider : MERGE / REJECT / REQUIRE_MORE_WORK.
4. Mettre à jour STATE.md (statut, décision) et incrémenter les compteurs.
5. Le gate de qualité (tests+lint) DOIT être vert avant MERGE.
6. Après MERGE : **CLEANUP obligatoire** — `git worktree remove <path>`, `git branch -d <branch>`,
   arrêt des instances prime-agent terminées (RAM finie, pas de zombie), puis mise à jour
   finale de STATE.md sous verrou `.agentloop/STATE.lock`.
Règle : modifications de lockfiles (Cargo.lock, gradle.lockfile, package-lock.json) et
écritures de STATE.md = sérialisées par l'orchestrateur, jamais en parallèle.
7. **Conflits au merge** : trivial (format/imports) → résolution par l'orchestrateur ;
touche la logique métier → résolution PUIS re-checker complet (gates rejoués) avant
MERGE ; sémantique non tranchable → REJECT + replanification/BLOCKED. Jamais de
résolution silencieuse sans preuve archivée.
