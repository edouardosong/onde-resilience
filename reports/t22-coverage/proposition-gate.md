# T22 — Proposition de gate coverage (DOCUMENT — pas d'implémentation CI)

**Statut** : recommandation. La décision d'adopter le seuil et la date d'activation appartient à l'orchestrateur / à l'utilisateur (règle v2.2 refusée : « une règle de coverage en gate sans outil = verdict subjectif »). Ce document pose l'outil (cargo-llvm-cov 0.9.0, déjà installé et mesuré) et la baseline factuelle (`baseline.txt`, même dossier).

## Baseline factuelle (core workspace, commit 6cf50db)

- TOTAL lignes : **89,80 %** (1194/11705 manquées) · régions 90,40 % · fonctions exécutées 82,86 %
- Étendue fichier par fichier : de **0,00 %** (`src/bin/node.rs`) à **98,50 %** (`crates/dtn-router/src/lib.rs`)
- Commande de référence : `cd core && cargo llvm-cov --workspace --summary-only` (exit 0, ~build debug + tests défaut)
- Coût machine constaté : une compilation debug complète au premier run ; runs suivants incrémentaux (rapide).

## Règle proposée (bornée sur fichiers modifiés)

**Gate « diff coverage » sur la PR, jamais un seuil global figé.**

1. **Périmètre** : seuls les fichiers `.rs` modifiés par la PR (diff vs branche de base) entrent dans le calcul. Les fichiers non touchés ne peuvent PAS faire échouer la gate — cela évite de pénaliser les zones historiquement faibles (`src/bin/node.rs` à 0 %) qu'une PR sans rapport ne répare pas.
2. **Métrique** : pourcentage de **lignes nouvelles/modifiées exécutées par les tests** (diff line coverage). Régions et branches : indicatif uniquement (llvm-cov rapporte 0 branche sur ce projet aujourd'hui).
3. **Seuil recommandé** : **≥ 80 %** des lignes du diff couvertes, avec **marge de tolérance ±2 pts** en warning (78–80 % = warning affiché dans la PR, < 78 % = gate rouge). Justification du choix : la moyenne existante est 89,8 % ; exiger 80 % sur le diff est atteignable immédiatement sans forcer l'écriture de tests cosmétiques, tout en remontant la queue de distribution (fichiers actuels sous 80 % : llm-inference 66,7 %, network 73,0 %, zim-parser/lib 75,2 %, ai/mod 77,3 %).
4. **Exclusions explicites** (à lister dans la config de la gate, pas cachées) :
   - `src/bin/*` (binaires CLI/glue — candidats e2e, pas unitaires),
   - code généré (ex. `crates/whisper_cpp_sys`, bindings),
   - blocs `#[cfg(test)]` eux-mêmes,
   - tests marqués `#[ignore]` : leur code n'est PAS compté comme couvert ; une PR qui déplace de la logique derrière un test ignoré verra sa diff-coverage chuter — comportement voulu.
5. **Outil de mesure en CI** : même binaire que la baseline — `cargo llvm-cov --workspace --summary-only --fail-under-lines 80` en mode global OU export lcov + script de diff (ex. `cargo-llvm-cov --lcov` + filtrage par `git diff --name-only base...HEAD`). Outil gratuit/open source (MIT/Apache-2.0), aucun service externe.
6. **Marges d'arbitrage humain** : toute exception (merge sous le seuil) exige une note dans `reports/<tâche>/` avec la valeur mesurée et la raison — traçable dans l'audit loop.

## Limites connues (factuelles)

- **Tests ignorés / flaky** : llvm-cov mesure l'exécution réelle ; un test `#[ignore]` ou qui panic avant ses assertions ne couvre rien. La gate diff-coverage pousse donc à « réactiver » plutôt qu'à empiler des tests morts — mais elle ne détecte pas un test qui asserte trop peu (mutation testing = complément, cf. T24).
- **Code généré & FFI** : `whisper_cpp_sys`, bindings llama.cpp : hors périmètre utile, à exclure explicitement sinon le % global est artificiellement tiré vers le bas.
- **`cfg(test)` / dead code autorisé** : les helpers de test comptent dans les lignes du diff ; risque mineur de « coverage de test par du test ». Acceptable, standard de l'industrie.
- **Multithreading/timing** (réseau DTN, idle server) : certaines lignes ne s'exécutent que sous conditions de timing ; un test déterministe peut laisser quelques lignes irréductibles → d'où la marge ±2 pts et l'arbitrage humain documenté.
- **Coût CI** : build debug + run complet des tests du workspace à chaque PR (~minutes) ; acceptable à l'échelle actuelle, à surveiller si le workspace grossit.

## Alternatives comparées

**Alternative A — seuil global figé (ex. « TOTAL ≥ 90 % lignes »)** : Simple à implémenter (`--fail-under-lines 90`) mais rigide : une seule grosse PR qui ajoute du code neuf non testé peut casser la gate pour tout le monde, et elle n'empêche pas une PR d'ajouter du code à 0 % tant que la moyenne tient. Elle punit aussi structurellement les fichiers binaires (node.rs 0 %) sans les corriger.

**Alternative B — mutation testing obligatoire sur le diff (ex. cargo-mutants)** : Beaucoup plus forte en garantie réelle (prouve que les tests TUENT les mutants, cf. preuve T24 : mutant health.rs:198 tué) mais coût prohibitif en CI systématique (minutes→heures selon module) et outillage plus fragile. Réservé aux modules critiques (crypto/auth/réseau/storage) en budget borné, comme déjà pratiqué (reports/t21-mutants, t23-mutants).

## Recommandation finale

Adopter la règle diff-coverage ≥ 80 % ±2 pts (périmètre : fichiers .rs modifiés, exclusions listées ci-dessus), mesurée par cargo-llvm-cov en CI, en gardant cargo-mutants ciblé sur les modules critiques. Ne PAS fixer de seuil global figé tant que `src/bin/node.rs` (118 lignes à 0 %) n'est pas traité par des tests e2e dédiés.
