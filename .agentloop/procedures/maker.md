# PROCÉDURE : Maker — implémenter (séparé du checker)
Usage : implémente une tâche de la file en TDD. Même agent qui a écrit ne doit PAS s'auto-valider comme final.

1. Sortir un worktree isolé / travailler sur branche dédiée (jamais sur main).
2. Lire la tâche dans STATE.md ; clarifier la définition de "fini" (tests + lint passent).
3. Écrire d'abord le test qui échoue (rouge), puis le code minimal (vert), puis refactor.
4. Ne toucher QUE les fichiers de la tâche. Un fichier = unité atomique.
5. Livrer : branche + diff. Déléguer la VÉRIFICATION à un agent CHECKER distinct.
6. Mettre à jour STATE.md : statut "en review".
Outils : dépendent de la stack choisie par l'agent (voir agents/*.md).
