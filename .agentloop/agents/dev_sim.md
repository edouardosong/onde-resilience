# Agent Dev Simulation — CHOIX EFFECTUÉ

Projet ONDE — développeur simulation (Python).

## Stack choisie
| Composant | Choix | Rôle |
|-----------|-------|------|
| Langage | **Python 3.11+** (3.11.16 présent) | indispensable — repo 100% Python |
| Calcul | **NumPy + SciPy** | indispensable — analyse statistique des résultats, metrics réseau |
| Simulation | **simpy** | **INDISPENSABLE** — la sim existante (`simulation/mesh_sim.py`) est déjà construite sur SimPy (discrete-event) ; se brancher sur l'existant |
| Tests | **pytest + pytest-benchmark** | indispensable — TDD sur moteur sim + benchmarks de perf |
| Plots | **matplotlib** | indispensable — visualisation délivrance/hops/latence pour l'analyse |
| Lint/Types | **ruff + mypy** | recommandé — qualité code en loop engineering |

## Justification en 1 ligne
SimPy est indispensable parce que le prototype `mesh_sim.py` utilise déjà SimPy —
réutiliser le moteur existant évite de réécrire la logique DTN/routage mesh, et toutes
les autres briques (NumPy/SciPy, pytest, matplotlib) servent à l'analyse des résultats
et au Q.A. de la boucle.

## Remarques (exploration)
- `simulation/mesh_sim.py` : simulateur mesh hybride SimPy (DTN store-and-forward,
  routage opportuniste, PoW antispam, transactions ZK, adressage Yggdrasil IPv6).
  Échelle par défaut réduite (10k mobiles / 1k bridges) — l'échelle cible annoncée dans
  le header est 500k/50k pour laquelle `encounter_opportunity` en O(n²) échantillonné
  à 500 serait à réécrire.
- `onde/simulation/results/simulation_report.json` : rapport existant (165s réels pour
  1h simulée). **Bug détecté** : delivery_rate_percent = 32953% (délivrés >> envoyés)
  car le compteur `total_messages_delivered` est incrémenté une fois par voisin direct
  sans dédupliquer, et les rencontres forward en boucle. Latence moyenne = 0.0 et
  PoW fail==success sondent à vérifier.

## Priorités de travail (proposition)
1. Corriger le bug de métrique (déduplication/délivrance) dans mesh_sim.py.
2. Benchmark perf (pytest-benchmark) sur l'échelle 500k nœuds.
3. Plots matplotlib : courbes délivrance, hops DTN, latence, charge buffer au fil du temps.
