# T25 — Phase 3.3 : Simulation 11k nœuds (stabilité + performance mesh)

**Tâche** : T25 · P2 · module `simulation/` · risque sécurité : NON · dépendances : NON (uv.lock intouché)
**Date** : 2026-08-23 · **Branche** : `loop/t25-sim11k` · **Maker** : data-sim (simulation)
**Verdict** : ✅ **11k nœuds ATTEINT — aucun goulot bloquant, aucune modification de `mesh_sim.py` requise.**

## Configuration des runs

- `mesh_sim.py` **non modifié** (zéro commit perf nécessaire) ; seed=42 fixée ; 3600 s simulées ;
  zone 10×10 km ; ratio mobiles:ponts = 10:1 à chaque palier.
- Le palier 11k (10 000 mobiles + 1 000 ponts) est exactement la config par défaut du `__main__`
  du simulateur.
- Driver de mesure : `bench_t25.py` (ce répertoire), lancé par
  `uv run --project simulation python reports/t25-sim11k/bench_t25.py <mobile> <bridge> 3600 42 <label>`.
- RAM pic : `resource.getrusage(RUSAGE_SELF).ru_maxrss`. p50/p95 latence : capture **additive**
  par monkeypatch de `SimStats.register_unique_delivery` (l'original est toujours exécuté ;
  même jeu d'échantillons que la moyenne du rapport — le modèle n'est pas modifié).
- Un seul job lourd à la fois ; budgets temps par palier jamais approchés (cf. tableau).

## Résultats par palier

| Nœuds | Wall-time (s) | RAM pic (Mo) | Delivery rate (%) | Latence moy. (s) | Latence p95 (s) | Throughput (msg/s sim.) | Stabilité |
|---:|---:|---:|---:|---:|---:|---:|---|
| 500    | 30,28 | 33,9 | 44,75 | 19,6   | 60    | 4,411 | exit 0, 0 exception |
| 2 000  | 30,90 | 34,2 | 46,14 | 117,8  | 585   | 4,314 | exit 0, 0 exception |
| 5 000  | 36,67 | 37,2 | 43,80 | 178,8  | 1 110 | 4,411 | exit 0, 0 exception |
| 8 000  | 39,78 | 41,7 | 42,14 | 176,6  | 1 265 | 4,413 | exit 0, 0 exception |
| **11 000** | **41,36** | **46,5** | **42,13** | **130,7** | **1 065** | **4,273** | **exit 0, 0 exception, 0 timeout** |

Critère d'acceptation « throughput/latence mesurés » : ✔ mesurés à chaque palier (tableau ci-dessus +
JSON complets dans `donnees-brutes.txt`). Budget palier 11k ≤ 15 min : consommé **41,4 s** (~3 %).

## Analyse de complexité (code lu avant scaling)

- **Rencontres : déjà O(1) vs N.** `encounter_opportunity()` échantillonne
  `min(500, len(nodes))` nœuds puis passe par `_encounter_pairs_tiered`
  (L2-14/ADR-001, bucketing multi-tier EXACT). Vérifié en lisant le code ET par la mesure :
  le nombre de paires par tick reste ~38–42 k à tous les paliers (échantillon fixe) — l'O(n²)
  naïf est bien traité, confirmé jusqu'à 11k.
- **Terme linéaire en N : `find_neighbors`** — scan O(N) par message envoyé dans `send_message`
  (~15,4–15,9 k messages/heure simulée, indépendant de N). Mesuré : **wall ≈ 29,7 s + 1,15 ms/nœud**
  (R² = 0,956 sur les 5 paliers).
- **Constante dominante : PoW adaptatif (~29 s)** — quand la charge dépasse le seuil, la difficulté
  monte à 8 et chaque message gaté (alert/aid) brûle MAX_ATTEMPTS = 10 000 sha256 (~0,34 µs/essai,
  bench séparé) sans succès ; visible dans les logs (`PoW OK` gelé à partir de t≈1920 s simulées).
  Ce coût est **indépendant de N** — il plafonne la courbe aux petits paliers mais ne bloque pas le scale-out.
- Autres termes O(N) secondaires : rebuild de la liste des receveurs ZK par transaction
  (`generate_transactions`, ~3,3 k×O(N)), création des nœuds, `move_nodes` par tick de monitoring.

### Décision perf
Le fix borné envisagé (index spatial pour `find_neighbors`, préservant l'ordre exact des voisins et
le flux PRNG) n'était **pas nécessaire** : 11k termine en 41 s, soit ~22× sous le budget de 15 min.
Aucun commit `perf(sim)` — donc aucun risque de dérive sémantique. La projection extrapolée
(NON mesurée, hors périmètre T25) donne ~87 s à 50k et ~11 min à 550k avec le code actuel.

## Déterminisme (L2-12)

Deux runs palier 500 (450+50), seed=42 : rapports JSON **byte-identiques** hors `real_time_sec`
(`diff` filtré → exit 0). Preuve rejouable : voir `donnees-brutes.txt` § DÉTERMINISME.

## Lectures réseau (sémantique simulation/README.md respectée)

- `delivery_rate_percent` ≈ 42–46 % à tous les paliers : la baisse intra-run vient du verrouillage
  PoW adaptatif (difficulté 8 → nouvelles alertes rejetées après t≈1920 s) — comportement du modèle,
  **indépendant de N**, hors périmètre T25.
- La latence moyenne et la p95 croissent avec la densité (plus de répliques DTN en transit → chemins
  store-and-forward plus longs avant première livraison) : cohérent avec la sémantique par-message
  de `register_unique_delivery`.
- Throughput émission ≈ 4,3–4,4 msg/s simulés, quasi constant vs N (générateurs de trafic bornés).

## Gates

- `uv run --project simulation pytest simulation/ -q` → **23 passed**, exit 0 (rejoué après travaux).
- Aucun changement `core/`, `ui/`, `android/`, `Cargo.*`, `uv.lock` ; aucune nouvelle dépendance Python.

## Artefacts

- `rapport.md` (ce fichier)
- `donnees-brutes.txt` — RESULT_JSON par palier + preuves déterminisme/stabilité
- `bench_t25.py` — driver de mesure rejouable
- `raw/<palier>_run.log.txt` + `raw/<palier>_report.json` — sorties complètes par palier (+ doublon 500bis)
