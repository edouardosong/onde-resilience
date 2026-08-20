# ONDE — simulation/ (SimPy mesh) — Sémantique des métriques

Documentation courte du simulateur `mesh_sim.py` : met l'accent sur la
**sémantique des métriques** (par-COPIE vs par-MESSAGE), une source durable
de confusion — y compris à l'origine du bug « taux de délivrance = 32953 % »
corrigé en L2-01.

## Règles d'exécution

```bash
uv sync --project simulation
uv run --project simulation pytest simulation/ -q
```

Déterminisme : `run_simulation(seed=42)` applique `random.seed(seed)` lui-même →
2 runs avec la même seed produisent des rapports **byte-identiques** (hors
wall-time). Défaut 42 = valeur utilisée par les tests/preuves (L2-12).

---

## Sémantique des métriques

Les compteurs du simulateur se répartissent en **deux familles** dont la
signification est volontairement différente. Les confondre fausse les lectures.

### Pourquoi deux unités ? — store-and-forward = N répliques par message

Le routage DTN est un **store-and-forward** : un message diffusé par son auteur
est **répliqué** sur chaque nœud qui le reçoit (buffer DTN), puis chaque
réplique est relancée à la rencontre d'un nouveau nœud. Une **seule information**
(logique) circule donc sous la forme de **N répliques physiques**.

- comptés **PAR-MESSAGE** : on dénombre les **informations distinctes**
  (clé `(sender_id, msg_id)`), chaque information ne compte qu'**1** fois.
- comptés **PAR-COPIE** : on dénombre les **événements réseau**, chaque
  réplique peut générer plusieurs événements pour la même information.

### Table de correspondance

| Métrique | Unité | Signification |
|---|---|---|
| `total_messages_sent` | par-MESSAGE | 1 par tentative d'émission (compte aussi les émissions rejetées par le PoW — l'incrément précède le check ; voir `pow_fail`/`pow_success` pour le détail) |
| `total_messages_delivered` | **par-MESSAGE** | 1 par information dont ≥1 copie a été livrée (dédupliqué `(sender, msg_id)`, L2-01) |
| `delivered_unique_messages` | par-MESSAGE | alias additif de `total_messages_delivered` (nom explicite) |
| `expired_unique_messages` | **par-MESSAGE** | 1 par information dont ≥1 copie a expiré (première copie expirée, L2-10) |
| `total_dtn_hops` | **par-COPIE** | 1 par **hop d'une réplique** (travail réseau) |
| `total_messages_expired` | **par-COPIE** | 1 par **copie** qui expire (plusieurs copies d'une même information peuvent expirer) |

`total_messages_delivered` (par-message) ≤ `total_dtn_hops` (par-copie) : le
nombre d'hôtes du réseau ne se confond jamais avec le nombre de messages.

### Pourquoi `expired`/`dtn_hops` sont par-copie et `delivered`/`sent` par-message

- **`delivered` et `sent` mesurent l'INFORMATION** : « mon alerte a-t-elle été
  émise ? a-t-elle atteint au moins un destinataire ? ». La réponse est oui/non
  par message — compter les copies le biaise (une alerte livrée à 50 voisins
  est comptée 50 fois → voir le bug 32953 %, corrigé en L2-01).
- **`expired` et `dtn_hops` mesurent le TRAVAIL réseau / la charge** : chaque
  réplique stockée, chaque relais est un coût (buffer, énergie, bande). La
  métrique adaptée à la charge est donc par-copie, et la même information leurre
  N fois.

### Anti-confusion : le taux de délivrance

`delivery_rate_percent = delivered_unique / sent × 100` reste dans [0, 100] %
(par-message des deux côtés). N'utiliser **jamais** `total_dtn_hops` ni
`total_messages_expired` au numérateur : leur unité (par-copie) n'est pas
compatible avec le dénominateur (par-message) et produirait des taux > 100 %
— c'était la cause racine du faux « 32953 % » avant le fix L2-01.

### Variantes par-message (L2-10, additives)

Deux variantes par-message ajoutées en L2-10, **sans modifier les compteurs
existants** (surcharge additive uniquement) :

- `SimStats.delivered_unique_messages` — propriété = `total_messages_delivered`
  (qui est déjà par-message). Nom explicite pour lever l'ambiguïté.
- `SimStats.expired_unique_messages` — compteur dédié, incrémenté seulement à
  la **première** copie expirée d'un message (`register_unique_expired`).

Toutes deux sont exposées dans le rapport JSON, champ `network_stats`.

---

## Rappel — rencontres (L2-04)

La recherche des paires de rencontre est EXACTE (même ensemble de paires, même
ordre) par bucketing spatial ou par double-boucle — choisie selon le régime via
`_encounter_pairs`. Les tests de régression `encounter_*` (test_mesh_sim.py)
prouvent l'équivalence NAIVE vs BUCKETÉ, le traitement du bord `dist = S`
(portée exacte) et l'absence de faux positifs hors-portée.
