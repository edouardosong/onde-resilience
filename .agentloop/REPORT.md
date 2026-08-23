# ONDE — RAPPORT DE SYNTHÈSE : STACK DES 9 AGENTS DE L'ÉQUIPE

> **Cycle 1 du loop engineering** — chaque agent a CHOISI sa stack parmi la liste proposée,
> **validée par exécution réelle sur le clone** de `/home/linux/onde-resilience-clone`, puis l'a
> déposée dans `.agentloop/agents/<role>.md` (section CHOIX EFFECTUÉ).
> Méthode : makers (dev_*) et checkers (tester_*/security) séparés — maker/checker split.
> Date : 2026-08-20 · Modèles : Qwen 3.8-27B local + DeepSeek V4 Flash cloud selon charge.

---

## 1. Récapitulatif des rôles et de leur stack

| Rôle | Type | Stack choisie (indispensables) | Justification clé |
|------|------|-------------------------------|-------------------|
| **tech_lead** | orchestrateur/arbitre | Orchestration `rlm()` + `/goal` + heartbeat ; revue de code manuelle ; arbitrage peer-review | décisions d'architecture + gates de sortie |
| **dev_sim** | maker (Python) | Python 3.11+, NumPy/SciPy, **SimPy**, pytest+pytest-benchmark, matplotlib, ruff+mypy | réutilise le moteur SimPy existant (`mesh_sim.py`) ; bug de métrique détecté (delivery 32953%) |
| **dev_frontend** | maker (Tauri/React) | React 19 + TS strict, Vite 7/8 + Tauri 2, Tailwind (AMOLED), Zustand, React Router v7, zod, Vitest+RTL, Playwright, ESLint+knip | typage des 8 commandes du pont Rust↔JS ; Vite/Tauri déjà amorcés |
| **dev_android** | maker (Kotlin) | Kotlin DSL, AGP 8.x, Jetpack Compose + Material 3, ViewModel/StateFlow, Hilt, Ktor client, JUnit5+Robolectric+MockK+Turbine, ktlint+detekt+Android Lint | remplace la WebView Java vanilla legacy ; DI compile-time ; gate qualité non-négociable |
| **dev_rust** | maker (Rust core) | cargo build/test, **clippy -D warnings**, fmt, rust-analyzer, proptest, llvm-cov, cargo-audit | 141 tests verts (baseline) ; module n°1 = `src/protocol/mod.rs` (porte d'entrée du trafic non fiable) |
| **tester_rust** | checker (Rust core) | cargo test, **proptest**, cargo-llvm-cov, cargo-fuzz (différé), criterion | invariants wire/DTN/crypto par propriété ; 141 tests existants comme référence de régression |
| **tester_android** | checker (mobile) | ADB, AVD/émulateur, UI Automator, Logcat, Maestro, Espresso, Monkey, JUnit+Robolectric, Perfetto | **constat honnête** : pas de /dev/kvm → outils device configurables mais non exécutables ; build+lint+aapt exécutés |
| **security** | checker (SecOps) | **cargo clippy --pedantic**, **cargo audit**, **detect-secrets** (+ OSV-Scanner/Dependabot/gitleaks/semgrep en CI, fuzz différé) | 0 warning, 0 vuln (139 deps), 0 vrai secret — audit 5 zones à risque |
| **devops** | gates/CI | GitHub Actions + matrix, **fastlane**, Docker/compose, CodeRabbit, coverage (llvm-cov/codecov), gh CLI | repo GitHub → Actions ; pipeline CI réel écrit et vérifié |

---

## 2. Ce que le cycle 1 a PRODUIT (au-delà du choix de stack)

1. **Pipeline CI complet — `.github/workflows/ci.yml` (devops)** : 3 jobs (rust / android / ui)
   + qualité hebdomadaire (cargo & npm audit). Least-privilege permissions, concurrency,
   matrix Rust NDK (aarch64/armv7/x86_64). Vérifié localement : 160+ tests, clippy/fmt verts.
2. **Manifest v2 de la boucle — `.agentloop/MANIFEST.md`** : séquence TRIGGER→TRIAGE→…→MÉMORISER,
   table brique (Osmani)→implémentation prime-agent.
3. **Audit sécurité détaillé — 5 zones à risque** avec ordre de traitement :
   DTN buffer sans cap (DoS) > parsing protocole unwrap/expect > dérivation crypto couplée >
   ZK MOCK > chaîne APK. + commande de non-régression à rejouer au gate.
4. **Findings sim** : bug métrique `delivery_rate_percent=32953%`, échelle O(n²), pas de scaffolding.
5. **Findings Android** : targetSdk 34→36, libs datées, dataExtractionRules.
6. **Probité technique de l'équipe** : tester_android a signalé l'impossibilité (pas de KVM) plutôt
   que de prétendre exécuter l'émulateur ; security a trié applicable / différé / non-applicable.

---

## 3. Boucle réellement en place (mapping loop-engineering → prime-agent)

| Brique du loop | Implémentation | Instance |
|---|---|---|
| Automations/heartbeat | `rlm_heartbeat` | label `onde-triage` (30 min, follow_up) |
| Goal persistant | skill `goal` | objectif actif + budget |
| Worktrees / isolation | sessions enfants `rlm()` (session_dir) | 8 enfants parallèles |
| Skills / contexte | `.agentloop/procedures/` + MANIFEST | triage/maker/checker/arbiter |
| Sub-agents maker/checker | `rlm()` + `agent_message` | 9 specs enregistrées (harness) |
| Mémoire externe (état) | **`.agentloop/STATE.md`** | survit aux runs ; backlog + compteurs |
| Règles d'arrêt | goal budget + 3 échecs→escalade + gates tests | documentées dans STATE.md |

---

## 4. Backlog priorisé pour le cycle 2 (détecté par l'équipe)

1. **[CRITIQUE]** DoS dtn-router : borner `payload` et `delivered_to`, vraie valeur de `max_buffer`.
2. **[CRITIQUE]** Remplacer `unwrap/expect` du parsing `protocol/mod.rs` par `Result` + caps.
3. **[HAUTE]** Bug métrique sim `delivery_rate` (dédup livraison) dans `mesh_sim.py`.
4. **[HAUTE]** Bat fixes sim : borrow / lien de la dérivation crypto Ed25519→X25519 (documenté).
5. **[MOYENNE]** Échelle O(n²) `encounter_opportunity` → 500k nœuds cible ; scaffolding (pyproject/venv).

Prochaine itération : triage (heartbeat) → sélection d'une tâche → maker → checker (split) → gate → merge → mémoriser dans STATE.md → décider continuer/arrêter selon budget + échecs.

---

*Généré à la clôture du cycle 1. Relire les fichiers `.agentloop/agents/*.md` pour le détail complet de chaque stack.*
