# ONDE — MANIFEST DU LOOP ENGINEERING v2 (adapté prime-agent)

> Recherche 2026-06 : **Addy Osmani** « Loop Engineering » (5 briques + état), **dualmedia.fr**
> (composants + règles d'arrêt), **Ralph Wiggum / ralph-playbook** (état-fichier entre itérations),
> ECC skills `autonomous-loops` + `continuous-agent-loop` (séquence maker→de-sloppify→verify→commit,
> boucle PR continue, RFC-DAG). « Le modèle oublie, le repo n'oublie pas. »

## 1. Architecture de la boucle — séquence d'une itération

```
TRIGGER ──> TRIAGE ──> SÉLECTION ──> ISOLATION ──> MAKER ──> CHECKER ──> GATE ──> MERGE ──> MÉMORISER ──> (encore ?)
 (heart-   (lit le    (tâche      (git         (agent    (agent      (tests    (orches-   (update      (si backlog
  beat /   STATE.md,   prioritaire worktree    MAKER     CHECKER     verts +   teur      STATE.md,     non vide
  goal)    détecte     + critères  isolé       unique,   UNIQUE,     lint +    seul à    LOG)           et budget
  )        )           d'accept.)         )    TDD)      ≠ maker,    Playwright)  merge)
                                                     |          |
                                                     +---- 2 échecs de gate -> BLOCKED + ESCALADE HUMAINE
```

## 2. Briques de la boucle → implémentation prime-agent/ONDE

| Brique (Osmani) | Rôle dans la boucle | Implémentation ONDE |
|---|---|---|
| **Automations** (heartbeat) | Découverte + triage sans humain | `rlm_heartbeat` (label `onde-triage`, 30 min, follow_up) + mode `goal` (run-until-done avec budget) |
| **Worktrees** | Isolation des agents parallèles | `git worktree add ~/onde-wt/<tâche>` (1 worktree = 1 tâche = 1 maker) |
| **Skills** | Connaissance projet, pas de re-découverte | `~/.agents/skills/onde-dev-team` (équipe+protocole), `onde-loop` (cette boucle), ECC : tdd-workflow, verification-loop, santa-method, security-review, rust-testing… |
| **Connectors/outils** | La boucle agit dans l'env réel | Playwright (MIT, E2E UI), cargo/gradle/adb, uv, Docker, git — installés et vérifiés |
| **Sub-agents** | **Maker ≠ Checker** (brique n°1 de la boucle) | 10 specs dans le harness prime-agent (voir §3) ; le worker qui écrit ne se grade JAMAIS |
| **État (spine)** | Mémoire entre runs | **`.agentloop/STATE.md`** (survit aux sessions) + `reports/` + mémoires du harness |
| **Règles d'arrêt** | Budget, échecs, permissions, arbitrage | STATE.md §8 (non négociables) + `goal` token budget |

## 3. Équipe (10 specs harness prime-agent — stacks choisies par chaque agent, 2026-08-18)

**MAKERS** (implémentent) : `architect` · `rust-core` · `android-dev` · `frontend-dev` · `data-sim`
**CHECKERS** (vérifient, ≠ makers) : `code-reviewer` (revue adversariale + rejeu CI) · `qa-engineer` (Playwright e2e) · `security-auditor` (crypto/secrets/OWASP)
**ENABLERS** : `devops-ci` (pipeline, releases) · `docs-writer` (README/ADR/CHANGELOG = vérité)

> Règle maker/checker : toute tâche a UN maker et AU MOINS UN checker différent.
> Code crypto/auth/finance → security-auditor obligatoire. UI → qa-engineer obligatoire.

## 4. Déclencheurs (cf. TRIGGERS.md)
- **Triage périodique** : heartbeat `onde-triage` (30 min) — lit STATE.md, détecte le travail, auto-archivage si vide.
- **Goal mode** : `goal.create(objectif, token_budget)` — itérations en série jusqu'à la condition de fin vérifiée (checker indépendant valide la fin).
- **Trigger utilisateur** : « continue la boucle ONDE » → reprise via STATE.md §0.

## 5. Invariants (jamais transgressés)
1. Un maker ne se valide jamais lui-même.
2. Le merge est l'acte exclusif de l'orchestrateur, APRÈS gate vert + verdict CHECKER.
3. Chaque itération termine par une mise à jour de STATE.md (preuve dans §5 DONE).
4. Les règles d'arrêt (STATE.md §8) priment sur l'élan de la boucle.
5. Honnêteté ONDE : MOCK étiqueté MOCK, zéro secret en clair, crypto réelle.
