# PROCÉDURE : Checker — vérifier indépendamment (maker/checker split)
Usage : reçoit un livrable du maker, le VÉRIFIE. Instructions différentes = lève les biais du maker.

1. Ne pas assumer la validité : relire les diff et les tests comme un réviseur strict.
2. Vérifier contre la tâche de STATE.md : le livrable fait-il THE chose demandée ?
3. Gates : tests (rouge qui passe), lint, absence de régression.
4. Checklist technique + sécurité :
   - [ ] compile / tests verts
   - [ ] scan secrets propre (`gitleaks detect`) ; aucun bump de dépendance mêlé au fonctionnel
   - [ ] pas de path-through inutile, pas de dépendance ajoutée sans raison
   - [ ] pas de durcissement contourné (auth, crypto, secret, entrée non fiable)
   - [ ] conformité conventions projet (skills)
5. Verdict : APPROVED / CHANGES_REQUESTED (avec raisons précises, fichiers + lignes).
6. Renvoyer à l'orchestrateur ; ne jamais fusionner soi-même. L'arbitre tranche.
7. **Quarantaine flaky** : test UI/Android/réseau rouge → relancer 2× immédiatement avant
   verdict. Passe au retry → marquer `flaky` dans STATE.md ; exiger du maker une correction
   DU TEST (timeout/retry/déterminisme), JAMAIS du code métier.
8. **Preuve négative automatisée** en priorité : `cargo mutants --in-place` ciblé sur les
   fichiers du diff (budget horaire borné, sortie archivée dans reports/) ; mutation/omission
   manuelle seulement si l'outil ne couvre pas le cas. Un test n'est valide que s'il PEUT
   échouer pour une mauvaise raison — pas de test tautologique, pas de test qui copie
   l'implémentation, snapshot test uniquement justifié.
9. **Conflit logique au merge** : si la résolution d'un conflit touche la logique métier,
   exiger une re-revue checker COMPLÈTE après résolution (gates rejoués) avant tout MERGE.
