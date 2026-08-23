# T26 — Preuves gate coverage diff ≥ 80 % ± 2 pts (implémentation CI)

**Date** : 2026-08-23 · **Maker** : devops-ci · **Branche** : loop/t26-coverage-gate (base 1d46bcd)
**Décision appliquée** : gate diff-coverage ≥ 80 % ± 2 pts, bornée aux fichiers `.rs` modifiés du workspace core/, exclusions explicites (`src/bin/*`, `whisper_cpp_sys`, `build.rs`, `cfg(test)` couvert par construction, `#[ignore]` non compté couvert).

## Livrables

- `scripts/diff_coverage.py` — Python stdlib uniquement. Entrée : JSON cargo-llvm-cov + liste .rs modifiés. Sortie : agrégat lignes pondéré sur fichiers modifiés uniquement, détail fichier par fichier, exit 1 sous seuil-tolérance, exit 0 en bande de tolérance (WARN affiché) et sur « nothing to measure » (jamais de faux rouge).
- Job `coverage-gate` dans `.github/workflows/ci.yml` — checkout fetch-depth 0, base de diff = PR base.sha / push event.before (skip passant si absente ou irrésoluble), cargo-llvm-cov via taiki-e/install-action épinglé par SHA réel, `cargo llvm-cov --workspace --locked --json`, exécution du script avec SEUIL=80 TOLERANCE=2, artifacts de triage uploadés en cas d'échec uniquement.
- Aucun changement Cargo.toml/Cargo.lock (outil strictement côté CI) — vérifié ci-dessous.

## Preuve négative locale — fixtures (reports/t26-coverage-gate/fixtures/)

| Cas | Agrégat mesuré | Attendu | Exit code obtenu |
|---|---|---|---|
| (a) sain 88,00 % (85 % + 100 %, node.rs 0 % exclu) | 88,00 % (220/250) | exit 0 | **0** (`case-a-healthy.exit`) |
| (b) fautif 70,00 % | 70,00 % (175/250) | exit 1 + fichiers listés | **1** (`case-b-failing.exit`) |
| (c) rien à mesurer (uniquement exclus/hors périmètre) | — | exit 0 « PASS — nothing to measure » | **0** (`case-c-nothing-to-measure.exit`) |

Sorties complètes : `case-*-*.out`. Le cas (b) liste bien chaque fichier manquant avec ses lignes (ex. `core/crates/example/src/lib.rs 70.00% 140/200`) et nomme le pire fichier.

## Test sur le VRAI rapport cargo-llvm-cov

Rapport régénéré localement : `cd core && cargo llvm-cov --workspace --locked --json --output-path /tmp/cov.json` (cargo-llvm-cov 0.9.0, exit 0).
- SHA-256 : `c16bb09b706590ed845c210d7ad6b5436f00ea184411d2f20d2f7203933adcae`
- Taille : 2391197 octets · TOTAL frais : 10 511 / 11 705 lignes = **89,79 %** (cohérent baseline T22 = 89,80 %)

| Test | Liste « modifiés » | Agrégat | Attendu | Exit obtenu |
|---|---|---|---|---|
| bon fichier (dtn-router) | `core/crates/dtn-router/src/lib.rs` | 98,50 % (328/333) | exit 0 | **0** |
| top-fail baseline (< 80 %) | llm-inference + network/mod + zim-parser/lib + ai/mod | 73,21 % (429/586) | exit 1 propre | **1** |

Sorties : `real-report-good.out/.exit`, `real-report-bad.out/.exit`.

## SHAs des actions GitHub utilisés (vérifiés via api.github.com/repos/<owner>/<repo>/commits/<ref> le 2026-08-23)

| Action | SHA épinglé | Résolution API | Déjà présent repo (T12–T16) |
|---|---|---|---|
| actions/checkout | `11bd71901bbe5b1630ceea73d27597364c9af683` | « Prepare 4.2.2 Release » (v4.2.2) | oui |
| dtolnay/rust-toolchain | `6c977a6ca4077a0ceb28ffbe03f59d46e9ac8772` | merge #182 (pin v1) | oui |
| actions/cache | `0057852bfaa89a56745cba8c7296529d2fc39830` | prepare-4.3.0 (v4.3.0) | oui |
| taiki-e/install-action | `ba47c86ac325773530516bb756137ac718732518` | « Release 2.86.5 » (2026-08-21) | oui (job rust, tool: cargo-llvm-cov) |
| actions/upload-artifact | `65c4c4a1ddee5b72f698fdd19549f0f0fb45cf08` | PR #662 (v4.6.0) | oui |

Aucun tag flottant, aucun SHA inventé — tous identiques aux pins existants de ci.yml, re-vérifiés en direct.

## Validation YAML

`python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"` → OK, jobs = [rust, android, ui, quality-scan, coverage-gate].

## Verrous respectés

- `git diff main --stat -- '*Cargo.lock*' '*Cargo.toml*'` → vide (voir sortie commit).
- Gate sans faux rouge : nothing-to-measure → PASS ; base de diff absente (premier push, schedule) → skip passant ; fichiers hors core/ ignorés et listés.
