# BRIEF MAKER — T5 (Phase 3.2) : Fuzzing cargo-fuzz (crypto, protocole, parsing)

Tu es le MAKER (rôle rust-core) de la boucle ONDE. Lis d'abord :
- /home/linux/.agents/skills/onde-dev-team/SKILL.md (protocole équipe)
- /home/linux/.agents/skills/tdd-workflow/SKILL.md et /home/linux/.agents/skills/rust-testing/SKILL.md
- /home/linux/onde-resilience-clone/.agentloop/STATE.md (§1 objectif, §3 T5 = ta tâche)

## Ta tâche (bornée)
Phase 3.2 ROADMAP : « Fuzzing : cargo-fuzz sur crypto, protocole, parsing | 0 crash exploitable (budget horaire) ».
Le repo n'a AUCUNE cible fuzz aujourd'hui (pas de répertoire fuzz/). Le code à couvrir est principalement dans le crate `onde-core` (core/, ex. core/src/node/mod.rs : parsing wire `from_wire_bytes`, signatures crypto, etc.) — identifie aussi les autres parseurs/crypto publics pertinents (dtn-router, zim-parser...).

## Worktree (travail UNIQUEMENT ici)
/home/linux/onde-wt/t5-fuzzing — branche `loop/t5-fuzzing` (base origin/main a51466e). Ne touche AUCUN autre répertoire du repo.
NB : un travail partiel existe déjà sur disque (répertoire `core/fuzz/` avec Cargo.toml + fuzz_targets/fuzz_target_1.rs, non commité) — examine-le et réutilise-le si correct, sinon corrige/remplace-le.

## Plan d'implémentation (décidé par l'orchestrateur)
1. **cargo-fuzz** : l'orchestrateur l'installe en arrière-plan (`cargo install cargo-fuzz --locked`, log /tmp/cargo-fuzz-install.log). Vérifie `cargo fuzz --version` ; s'il n'est pas encore dispo, attends la fin de l'installation (ne relance pas d'install en parallèle).
2. Crée les cibles fuzz (convention cargo-fuzz : répertoire `fuzz/` avec sa propre Cargo.toml, hors workspace) couvrant AU MINIMUM 3 familles :
   - **protocole/parsing** : toutes les fonctions de parsing d'entrée non fiable (ex. `from_wire_bytes`, décodage messages DTN, tout `parse_*` public) ;
   - **crypto** : vérification de signatures / dérivation de clés / tout API crypto public acceptant des octets arbitraires ;
   - **parsing divers** : autres parseurs (zim-parser, etc.) si accessibles sans dépendance lourde.
   Chaque cible doit être simple et robuste (pas de panique sur `unwrap`/`expect` dans le chemin fuzzé — corrige les paniques triviales rencontrées).
3. **Exécution** : lance chaque cible avec un budget total d'au moins 45 min (ex. `cargo fuzz run <cible> -- -max_total_time=900` par cible, en parallèle si possible) depuis le répertoire approprié. Enregistre les sorties (nb de cas exécutés, crashes).
4. **Si crash** : analyse — bug réel → fix + test de régression ; sinon documente pourquoi ce n'est pas exploitable. DoD = 0 crash exploitable.
5. TDD/qualité : `cargo build --workspace` OK, `cargo clippy --workspace --all-targets` 0 warning, `cargo test --workspace` VERT (aucune régression). Commite sur loop/t5-fuzzing (message type(scope): sujet, ex. test(fuzz): ...). NE merge rien, NE push rien.

## Réponse finale
Via `await agent_message.send(message, receiver_role='parent')` : ce qui a été fait, les cibles créées + leurs sorties d'exécution clés (durée, nb cas, 0 crash), les 3 commandes du DoD avec sorties, la liste des commits (git log --oneline), et tout écart/risque connu. Si tu bloques (cargo-fuzz indisponible après attente, cible impossible à écrire...) : envoie immédiatement un message avec l'erreur exacte plutôt que de tourner en rond.
