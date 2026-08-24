# ONDE — T32-B · Démo mesh TCP réel sur appareils (bidirectionnelle, sans serveur)

**Date** : 2026-08-24 · **Repo** : /home/linux/Documents/CleanOnde · **Branche** : fix/e2e-bugs-t32b
**Binaire** : `onde_node` aarch64 release, statique, NDK r28c (2 709 424 o), cross-build local
(recette CI ci.yml job android : linker/CC/AR `aarch64-linux-android21-clang`, build 8,5 s à chaud).
**Appareils** : A = Xiaomi 11T « lisa » (Android 13) 192.168.1.98 · B = Pad 5 « nabu » (Android 15) 192.168.1.87
**Transport** : Wi-Fi LAN domestique, sockets TCP directs (aucun serveur, aucun internet dans le chemin mesh).
Exécution du binaire depuis `/data/local/tmp` via adb shell (domaine SELinux `shell`) — OK sur les deux.

## Identités (persistées SQLite meta.identity_seed, ed25519 RFC 8032)
| Rôle | Appareil | pubkey |
|---|---|---|
| A3 | lisa | `69db0cdf14ba1bd9344bdad72d00001edc288639cdcf1395842238c1fd48da09` |
| B3 | nabu | `fc0aff478e585932adaebfdbdd06a30c3021ec889aeee3998708df6cb6b96943` |

Validation croisée dérivation offline (python cryptography) ↔ log startup `pubkey=…` : identique octet/octet
(courses précédentes : `048f4d93…`/`899c01ae…`, logs/a-boot.log, logs/b-boot.log).

## Course 3 — preuve principale (logs/a3-listener.log, b3-publisher.log, a4-publisher.log, b4-listener.log)
Séquence ordonnée : A écoute `--listen 0.0.0.0:9333 --trust <pubB>` D'ABORD, puis B démarre avec
`--peers 192.168.1.98:9333 --trust <pubA> --publish "…"` (flags T32-B commit b4b3435).

### Direction 1 : nabu → lisa
- Publication nabu : id `30011a371f1d686a3e098878eafcbc1d7bc096258aba9cddd7f15083609e1f4c`,
  kind=Alert, signature_valid=true, pubkey=fc0aff… (log b3-publisher.log)
- tcp_connected peer=192.168.1.98:9333 ; /health lisa : ingested=1, rejected=0,
  peers known=1 synced=1, storage.events=1
- SQLite lisa (logs/a3.db) : ligne unique id=`30011a37…`, tier=critical ;
  payload deflate → contenu exact « ONDE T32B-R3 preuve finale sans internet nabu-vers-lisa »

### Direction 2 : lisa → nabu (rôles inversés, mêmes bases => WoT cumulée)
- Publication lisa : id `9015a9d45ae8e0e3971a104ce6cfaced067aca7667e0776ba39ce6f640f82f5c`
- /health nabu : ingested=1, rejected=0, storage.events=1
- SQLite nabu finale (logs/b3.db) : **LES DEUX événements** `30011a37…` ET `9015a9d4…` (tier critical)

## Interprétation
- Alertes signées Ed25519 échangées entre deux nœuds complets sur TCP réel inter-appareils,
  admises par le gate complet (signature + PoW adaptatif + réputation WoT bootstrappée),
  stockées hiérarchiquement et persistées SQLite des deux côtés.
- Authorité d'auteur prouvée par : gate d'admission (re-vérification signature côté récepteur,
  0 rejet sur les 2 directions) + IDs cohérents émission/réception + log publisher signé.
  (La table `messages` stocke le contenu compressé, pas le wire signé — choix de schéma existant.)

## Anomalie observée (courses 1-2, à ticket — non bloquante pour la preuve)
Course 1 : `--publish` émis ~7 ms AVANT l'établissement TCP → trame partie (B sent=1, A received=1)
mais NON stockée chez A (silence total, aucun compteur visible sans --health-port).
Course 2 : la même alerte `f2eefa93…` a alors été livrée via store-and-forward DTN à la reconnexion
(preuve de résilience DTN !), mais le second `--publish` (8422) n'est apparu NI chez A NI en local B.
Suspicion : course publish↔flush_outbound au premier dial + persistance locale du publish non systématique.
À reproduire en test e2e localhost (ordre publish/pré-connect) puis fix dédié (T32-C proposé).

## Non-objectifs de cette itération
- Isolation hotspot sans internet : coupe le adb sans fil (canal de contrôle) → nécessite USB ou
  2ᵉ interface ; reporté Phase C. Le trafic mesh est de toute façon socket-direct hors internet.
- UI Android (com.onde.resilience) : testée séparément (reports/e2e-bugs-t32b.md, 11/11 PASS).

## Repro
```bash
# build (recette CI) puis push
adb -s <SERIAL> push core/target/aarch64-linux-android/release/onde_node /data/local/tmp/
# bootstrap identité
adb shell 'timeout 4 ./onde_node --db t.db' ; adb pull t.db ; python: ed25519(seed)->pubkey
# direction X->Y
Y: nohup ./onde_node --db y.db --listen 0.0.0.0:9333 --health-port P --trust PUB_X
X: nohup ./onde_node --db x.db --peers IP_Y:9333 --trust PUB_Y --publish "msg"
# vérif : curl /health Y ; adb pull y.db ; python sqlite+zlib
```
