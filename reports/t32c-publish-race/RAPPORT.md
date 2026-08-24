# T32-C — course `--publish` ↔ premier dial TCP (maker ox-alpha, 2026-08-24)

Base : origin/main = 3a76093. Branche : `loop/t32c-publish-race` (local, pas de push/merge).

## Verdict sur les suspicions de l'incident

### (a) « flush_outbound peut partir avant que publish_alert ait alimenté pending » — NON REPRODUCTIBLE
Ordre du binaire (`core/src/bin/node.rs`) : `transport.start()` (dials) →
`publish_alert().await` → boucle de pump. Le publish précède STRICTEMENT toute
passe de flush (même tâche tokio). Preuve e2e déterministe :
`tests/t32c_publish_race_e2e.rs::publish_before_dial_is_delivered_after_settlement`
— dial en cours, publish AVANT tout pump, livraison garantie (mémoire + SQLite +
ingested=1) en < 1 s. 5/5 exécutions vertes SUR MAIN non modifié.

Le marquage « livré » à l'enfilement (tcp.rs `flush_outbound`) est sûr au sein
d'un run : la queue sortante survit aux déconnexions et `writer_loop`
re-enfile en tête sur erreur d'écriture ; après restart, la carte `delivered`
meurt avec le processus ⇒ le store-and-forward DTN re-propose l'événement.
Aucun changement : zéro régression DTN.

### (b) « --publish ne passe pas par le même persist » — FAUX
`publish_alert` (node/mod.rs:746) et `handle_incoming_alert` (node/mod.rs:918)
appellent le MÊME `persist_message` (tier Critical). Vérifié par lecture.

### Racine réelle (course 1) — refus d'admission SILENCIEUX par asymétrie de confiance
- Chaque nœud s'auto-confie (`Node::new`, GENESIS_TRUST=0.8 ≥ 0.7) ⇒ il publie
  avec `pow_difficulty = 0`.
- Un receveur sans notre clé dans SON trust exige MAX_POW_DIFFICULTY=4 pour un
  auteur inconnu (`reputation/mod.rs::required_pow_difficulty_inner`,
  score inconnu = UNKNOWN_TRUST=0.0).
- ⇒ `validate_with_pow_min` rejette (« PoW difficulty 0 is below network
  minimum »), violation InsufficientPow enregistrée contre l'auteur, issue
  `Rejected` comptée UNIQUEMENT en métriques mémoire — AUCUN log, aucun
  compteur visible : exactement le « silence total » observé (flags de
  l'incident sans `--trust` côté récepteur).
- La frame traverse proprement le réseau (sent/received, zéro violation
  framing) — preuve e2e :
  `untrusted_receiver_rejects_difficulty_zero_alert_visibly`.
- Course 2 (re-livraison stockée) : cohérent avec une cause DÉPENDANTE DE L'ÉTAT
  de A (confiance/skew horloge), pas de la frame — la même frame rejetée puis
  acceptée implique un changement côté A entre les deux livraisons. Non
  tranchable depuis les seuls logs fournis ; le mécanisme gate reste le seul
  reproductible en code.
- Publication disparue 7bf99225 côté B : `publish_alert` persiste toujours en
  local (Critical toujours retenu) SAUF doublon d'id ou échec SQLite (warn
  explicite). Absence des deux SQLite ⇒ hypothèse la plus probable : run sans
  `--db` ou id déjà connu ; rien dans le code ne permet une perte locale après
  `event=alert_published`.

## Correctif minimal (zéro wire, zéro DTN, panic-free)
1. `core/src/network/tcp.rs::process_inbound` : capture de la première raison
   de refus + UNE ligne structurée `warn! event=tcp_admission_rejected` par
   passe de pump (bornée par la cadence du pump, pas par le volume).
   L'antidote du « silence total ».
2. `core/src/bin/node.rs` : garde opérateur NON bloquante
   `warn! event=publish_without_trust` quand `--publish`+`--peers` sans
   `--trust`, avec la clé à déclarer chez les pairs.

## Gates locaux (worktree, tous exit 0)
- `cargo test --workspace --locked` : 346 passed / 0 failed
  (344 baseline + 2 nouveaux ; baseline archivée avant patch : 344/0)
- `cargo clippy --workspace --all-targets --locked -- -D warnings` : OK
- `cargo fmt --all -- --check` : OK
- `gitleaks detect --no-git` : no leaks found

## Appareils (lecture seule)
2 appareils adb connectés ; `pidof onde_node` vide sur les deux (aucun zombie).
