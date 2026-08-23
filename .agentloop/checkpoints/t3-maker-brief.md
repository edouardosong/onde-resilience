# BRIEF MAKER — T3 (Phase 2.1) : LLM local réel (llama-bind → GGUF)

Tu es le MAKER (rôle rust-core) de la boucle ONDE. Lis d'abord :
- /home/linux/.agents/skills/onde-dev-team/SKILL.md (protocole équipe)
- /home/linux/.agents/skills/tdd-workflow/SKILL.md et /home/linux/.agents/skills/rust-testing/SKILL.md
- /home/linux/onde-resilience-clone/.agentloop/STATE.md (§1 objectif, §3 T3 = ta tâche)

## Ta tâche (bornée)
Phase 2.1 ROADMAP : « LLM local réel : llama-bind → décodeur GGUF (llama.cpp), inférence sur-device | Question → réponse cohérente, RAM bornée ».
Le crate `core/crates/llama-bind` est actuellement en mode MOCK (feature `default = ["mock"]`, feature vide `llama-cpp`) : `LlamaContext::load`/`generate` retournent Err("Real llama.cpp bindings not yet implemented...") hors mock. 5 tests, tous sur le chemin mock.

## Worktree (travail UNIQUEMENT ici)
/home/linux/onde-wt/t3-llm-local — branche `loop/t3-llm-local` (base origin/main 51d713e). Ne touche AUCUN autre répertoire du repo.

## Plan d'implémentation (décidé par l'orchestrateur)
1. Ajoute la dépendance optionnelle `llama_cpp_sys` (version "0.3", nom du crate avec underscores) activée par la feature existante `llama-cpp` (Cargo.toml de llama-bind : `[dependencies] llama_cpp_sys = { version = "0.3", optional = true }`). Le default reste `mock`. NB : le build de cette crate compile llama.cpp (plusieurs minutes au 1er build) — c'est normal, ne l'interromps pas. **IMPORTANT (spécificité machine)** : tous les builds/tests cargo touchant la feature `llama-cpp` DOIVENT être lancés avec l'env var `BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-linux-gnu/15/include"` (sinon bindgen échoue sur stdbool.h). Exemple : `cd core && BINDGEN_EXTRA_CLANG_ARGS="-isystem /usr/lib/gcc/x86_64-linux-gnu/15/include" cargo test -p llama-bind --features llama-cpp`.
2. Implémente le FFI réel derrière `#[cfg(feature = "llama-cpp")]` :
   - `LlamaContext::load(model_path)` : chargement GGUF via llama-cpp-sys (petit contexte, n_ctx borné ex. 512–1024 → RAM bornée).
   - `generate(prompt)` : pipeline réel tokenize → decode → sampling (boucle jusqu'à max_tokens ou stop), retourne un `GenerationResult` avec des valeurs RÉELLES (n_tokens, gen_time_ms, tokens_per_sec, prompt_tokens).
3. Garde le chemin mock intact (feature default) : `cargo test --workspace` doit rester vert SANS fichier modèle (CI GitHub inchangée).
4. Ajoute des tests derrière la feature `llama-cpp` : chargement de `/home/linux/onde-models/qwen2.5-0.5b-instruct-q4_k_m.gguf` + génération d'une réponse courte à « Qu'est-ce que la RCP ? » — asserts : texte non vide, n_tokens > 0, tokens_per_sec > 0. Si le fichier modèle est absent : `eprintln!` explicite + skip (retour) pour que `cargo test --features llama-cpp` passe sur une machine sans modèle.
5. TDD autant que possible ; code propre, clippy 0 warning.

## DoD (preuve à fournir dans ta réponse)
- `cd /home/linux/onde-wt/t3-llm-local/core && cargo build --workspace` OK
- `cargo clippy --workspace --all-targets` : 0 warning
- `cargo test --workspace` : vert (features default, mock) — aucune régression
- `cargo test -p llama-bind --features llama-cpp` : VERT avec le modèle présent → inférence réelle question→réponse cohérente (copie la sortie du test dans ta réponse)
- Commits sur `loop/t3-llm-local` (message type(scope): sujet, ex. feat(llama-bind): ...). NE merge rien, NE push rien.

## Réponse finale
Via `await agent_message.send(message, receiver_role='parent')` : ce qui a été fait, les 4 commandes ci-dessus + leurs sorties clés, la liste des commits (git log --oneline), et tout écart/risque connu. Si tu bloques (build FFI impossible, API llama-cpp-sys introuvable...) : envoie immédiatement un message avec l'erreur exacte plutôt que de tourner en rond.
