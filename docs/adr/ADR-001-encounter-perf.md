---
status: ACCEPTÉ
date: 2026-08-20
deciders: maker L2-14 (ARCHITECT+MAKER), checker LOCAL indépendant
---
# ADR-001 — Performance & sémantique de la recherche de rencontres (`encounter_opportunity`)

## Contexte
Issue de la RECO checker L2-04. Le vrai goulot de la rencontre est le **régime dense** :
le bucketing L2-04 utilise **une seule taille de cellule = max portée** (ETHERNET bridge =
999 999 m). Dans un domaine 10 km (≪ S), tout tient dans ~1 cellule → la fenêtre 3×3
couvre tout → le bucketing dégénère en **Ω(m²) pour TOUS (bridges ET nœuds)** — d'où la
sortie de **124 750 appels `forward_opportunity`/tick** (= C(500,2)) dans le rayon bridge.
Voir ci-dessous (Annexe A) la preuve par mesure du coût par scénario.

## Décision
**Option (b) retenue : bucketing multi-anneaux / multi-tiers (un index spatial PAR TIER de
portée), sémantique préservée.** L'option (a) (cap/dédup) est **documentée mais NON
implémentée** (changement sémantique → décision produit requise, voir §4).

## Alternatives évaluées

### (a) Cap / dédup des rencontres — REJETÉE (changement sémantique, décision produit requise)
Limiter le nombre de paires traitées par nœud/tick.
- **Impact sémantique** : on DROPPE des paires vraies → l'ordre de consommation du PRNG,
  les mutations DTN (`forward_opportunity` non commutative) et la propagation changent.
  Déterminisme sous seed conservé mais **résultats différents** de la référence.
- **Équité/déterminisme** : quelles paires garder ? par nœud ? quel tick ? faut-il un
  ordre stable (ex. plus proches d'abord) ? Aucune réponse sans décision produit.
- **Gain** : seul capable de réduire le nb de forward calls dans le régime dense (jusqu'à
  124 750 → plafond choisi). MAIS changer la portée bridge / la densité d'échantillonnage
  est un autre levier, lui aussi produit.
- **Verdict** : levier efficace mais **sémantiquement risqué** → à trancher hors cette itération.

### (b) Bucketing multi-tiers — RETENUE (sémantique préservée, équivalence prouvable)
Un index spatial **par tier de portée** : grille fine pour les nœuds (200 m / 50 m), grille
grossière pour les ponts (999 km). Chaque paire est recherchée dans la grille de sa portée
max **R* = max(range(a), range(b))** ; l'exactitude tient au fait que `dist(a,b) ≤ R*` ⇒
`cell_R*(a)` et `cell_R*(b)` sont dans une même/voisine cellule (fenêtre 3×3).
- **Sémantique** : PRÉSERVÉE — **même ensemble de paires, même ordre (i, j)** que la
  référence double-boucle `_encounter_pairs_naive`. Preuve : 2000 configurations aléatoires
  (tiers mixtes, densités, domaines, coords négatives) + cas contrôlé L2-10 + cas dense,
  ensemble ET ordre identiques (voir Annexe B), verrouillés par les tests.
- **Perf** : enumérateur **sous-quadratique** en régime à portées étroites (~6-7× à grande
  échelle sur nœuds WIFI/BLE uniformes) ; sur le mix tech réaliste (WIFI/BLE/LORA+bridge)
  gain **~1.7× à 1.9×** réel et constant (cf. Annexe A). Ne réduit PAS les forward calls
  des paires **vraies** (densité bridge/LORA = propriété du modèle, pas un défaut
  d'énumérateur).
- **Coût mémoire** : K ≤ 4 grilles (une par portée distincte), chaque nœud indexé dans
  chaque grille d'une portée ≥ la sienne → O(m·K) entrées, négligeable.
- **Verdict** : sûr, additif, prouvable, améliore le facteur constant réellement.

### (c) Combinaisons / autres
- Tiered + préfiltre de la fenêtre par portée (déjà intégré à (b) via les listes indexées
  par `r_j` dans chaque cellule — évite de rescaner la grille grossière d'un pont).
- Cap (a) + tiered (b) pourraient se combiner plus tard (décision produit d'abord).

## Équivalence à prouver (si option (b))
1. `_encounter_pairs_tiered(positions) == _encounter_pairs_naive(positions)` **en ensemble
   ET en ordre** (i ascend, j ascend).
2. Le dispatch `_encounter_pairs` (chemin tiered nominal) reproduit la référence.
3. `encounter_opportunity()` end-to-end reste déterministe sous seed.
→ Tests `test_encounter_tiered_*` ajoutés à `simulation/test_mesh_sim.py`.

## Note sur le vrai goulot (transparence)
La mesure (Annexe A) montre que le coût `forward_opportunity` est **proportionnel aux paires
vraies**, majoritairement issues des portées LORA (5 000 m) et surtout ETHERNET (999 999 m).
`encounter_opportunity` échantillonne par défaut `min(500, n)` nœuds ; à 500 nœuds le
double-boucle vaut ~20.8 ms (déjà déployé tick/tick). L'option (b) le réduit à ~11 ms (×1.9)
sans casser la sémantique. Si l'exigence produit est d'atteindre un régime dense MAÎTRISÉ
(moins de forward calls), la seule option est (a) — cap — ou une décision sur la portée/la
densité d'échantillonnage : **ce sont des choix de produit, livrés ici comme plan, non
implémentés**.

## Annexe A — Preuve par mesure (où se perd le temps)
Repro `/tmp/l2_14_bench_repro.py` (simulation/, venv, worktree l2-14) — différentiel
`_encounter_pairs_naive` vs `_encounter_pairs_tiered`, positions réelles `add_mobile_node`/`add_bridge_node`.

Mélange tech réel (WIFI 200 m, BLE 50 m, LORA 5 000 m mobiles ; ETHERNET 999 km bridges) — 10 km² :
```
  n     naive_ms  tiered_ms   pairs     speedup
  500     20.799     10.952    24150      1.9×
 2000    303.948    183.853   351138      1.7×
 5000   1892.204   1089.890  2118469      1.7×
10000   7593.998   4376.637  8333110      1.7×
20000  30617.493  18188.064 33341886      1.7×
```
Nœuds uniformes WIFI/BLE sans LORA (portées étroites) — le gain monte à ~6-7× :
```
  n    naive_ms  tiered_ms   speedup
  500     14.297      2.511     5.7×
 2000    233.647     34.730     6.7×
10000   5922.080    794.892     7.5×
```
Coût par scénario (`encounter_opportunity` end-to-end, buffers remplis) avant/après :
```
 500mob   : 26.21 ms → 18.34 ms   (19 742 paires)
 490m+10br: 28.74 ms → 20.41 ms   (24 150 paires)
 400m+100br:41.53 ms → 38.76 ms   (57 777 paires)
 250m+250br:58.32 ms → 64.95 ms   (99 108 paires, forward domine)
 0mob+500br:70.46 ms → 72.55 ms   (124 750 paires, toutes vraies → forward non réductible)
```
Le régime « tout bridge échantillonné » (124 750 forward calls) est un maximum sémantique :
toutes ces paires sont **vraies** (portée 999 km ≫ domaine). Ne se réduit que par (a).

## Annexe B — Preuve d'équivalence
- `/tmp/l2_14_tier_proto.py` : **2000 configurations aléatoires** (2-40 nœuds, technologies
  aléatoires, domaines 100 m–100 km, coords positives ET négatives) — `set` ET `ord` de
  tiered == naive à 100 %.
- Cas contrôlé L2-10 (`ENCOUNTER_POSITIONS`, bord `dist=S`, asymétrie BLE/WIFI, nœud
  lointain) : identique.
- Cas dense all-bridge (500 ponts) : identique.
- Verrouillage par les tests (noms réels, simulation/test_mesh_sim.py) :
  - `test_encounter_tiered_matches_naive_sequence` (60 configs aléatoires
    déterministes : ensemble ET ordre identiques),
  - `test_encounter_tiered_bridge_present_matches_naive` (cas contrôlé petit
    domaine + bridge),
  - `test_encounter_tiered_boundary_dist_equal_and_no_false_positive` (bord
    `dist = S` + chaîne fermée faux positifs/faux négatifs),
  - `test_encounter_tiered_bucketed_naive_all_agree` (NAIVE==BUCKETÉ==TIERED
    en régime bucketing fin),
  - `test_encounter_dispatch_uses_tiered_equivalent` (dispatch + spy d'appel
    vers le chemin tiered + déterminisme end-to-end),
  - `test_encounter_tiered_exact_boundary_multiples_no_false_negative`
    (RÉGRESSION FP : coords = multiples EXACTS de R avec dist = R — cf. point
    de revue CALL-A, correctif `x/R`) — appliqué aussi à `_encounter_pairs_bucketed`.

### Annexe B2 — Correctif exactitude FP (revue CALL-A)
- Faux négatif démontré : `math.floor(x * (1.0/R))` (multiplication par inverse)
  décale d'une cellule de 2 quand `x` est un multiple exact de `R` ET `dist = R`.
  Contre-exemple : A=(15999984,0) WIFI, B=(16999983,0) ETHERNET (R=999999)
  → `x_a * inv_R` = 15.999999999999998 (floor 15) alors que `x_a / R` = 16.0
  (floor 16) → (a,b) hors fenêtre 3×3 → `naive=[(0,1)]` mais `tiered=[]=bucketed`.
- Correctif : division DIRECTE `x / R` (et `y / R`), aux 2 endroits du chemin
  tiered (construction de grille + requête, axes x ET y) ET dans le chemin
  bucketing L2-04 (`x / S`). Après correctif : tiered == bucketed == naive ==
  [(0,1)] sur le contre-exemple (x et y). 0 divergence sur 100 configs aléatoires.

## Historique
- L2-04 : bucketing grille unique S = max range (« vers le bas » en régime petit devant S).
- L2-10 : 4 tests de régression NAIVE vs BUCKETÉ (bord, faux positifs, dispatch, déterminisme).
- L2-14 : cet ADR — option (b) implémentée, option (a) documentée/non implémentée.
- L2-14 (revue CALL-A) : correctif exactitude FP `x/R` (multiples exacts de R, dist=R),
  appliqué aux chemins tiered ET bucketing, + test de régression dédié.
