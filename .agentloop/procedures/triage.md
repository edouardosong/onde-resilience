# PROCÉDURE : Triage de la file de travail
Usage : à chaque heartbeat / début de cycle.

1. `cd /home/linux/onde-resilience-clone && git status --short && git log --oneline -5`
2. Lister les issues ouvertes : `gh issue list` (si gh configuré, sinon lire TODO/backlog dans le repo).
3. Récupérer les tests cassés : `cargo test`, `pytest`, tests android s'ils existent.
4. Consolider dans `.agentloop/STATE.md` §3 (File de travail) : # | Priorité | Tâche | Statut | Assigné | Artefact.
5. Dédupliquer : ne pas re-ajouter une tâche déjà listée ou déjà en cours.
6. **Hygiène** : `git worktree list` + `git branch` — signaler puis supprimer les worktrees
   et branches zombies (tâche déjà mergée/rejetée non nettoyée) ; vérifier la RAM libre
   avant tout nouveau spawn.
7. **Santé du repo (fin d'itération)** : compter et consigner dans STATE.md (§2/§4) —
worktrees actives, branches orphelines, lockfiles modifiés non mergés, tests flaky connus,
tâches BLOCKED anciennes (>2 itérations), modules à échecs répétés. Entropie visible =
entropie traitable ; invisible = elle s'accumule.
Règle : un finding doit être MESURABLE et borné (fichier/module précis) pour entrer dans la file.
