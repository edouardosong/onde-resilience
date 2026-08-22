---
status: ACCEPTÉ
date: 2026-08-22
deciders: maker T13 (boucle ONDE itération 6, portage Fusion)
---
# ADR-002 — Extension du format wire pour les événements sociaux Tuitter/Redit (codes 16..21)

## Contexte

Le projet Fusion (social **Tuitter** micro-blog + **Redit** agrégateur par communautés)
apporte six nouveaux types d'événements mesh : publication, commentaire, vote,
abonnement, message privé et signalement de modération. Dans le clone source Fusion
(base `1fe33a1`), ces kinds étaient numérotés **15..20** — mais `main` a intégré entre-temps
la Phase 2.7 anti-abus qui occupe **déjà le code 15** pour `AbuseReport`
(endossement négatif, voir commit `9fe1577`). Un portage naïf aurait créé une collision
wire : deux sémantiques pour le même octet de kind, indécodable de façon cohérente.

Contraintes du format (établies par les phases 1.1/1.2/1.4/2.7) :

1. Les codes de kind font partie de l'**ID canonique signé** — renuméroter un kind
   existant invaliderait tous les événements historiques signés avec l'ancien code.
2. Un pair ancien doit **échouer proprement** (fermé) sur un kind inconnu :
   `MeshEvent::from_wire_bytes` retourne `Err("wire: unknown kind code …")`.
3. Tout événement social traverse le **gate d'admission** `admit_peer_event` (T10) :
   signature → auto-relais → déduplication → auteur ignoré → fenêtre glissante.

## Décision

1. **Renumérotation ADDITIVE** : les six kinds sociaux sont décalés de Fusion 15..20
   vers **16..21** (`SocialPost`=16, `SocialComment`=17, `SocialVote`=18,
   `SocialFollow`=19, `SocialMessage`=20, `SocialModeration`=21). Aucun code
   existant 0..15 ne change. Le schéma historique est respecté : signature Ed25519
   sur l'ID canonique, PoW adaptatif selon la réputation de l'auteur, échec fermé
   sur kind inconnu.
2. **Test de stabilité étendu** : le test wire verrouille désormais la table complète
   `[0..21]` (codes exacts, unicité, roundtrip signé+PoW pour chaque kind social,
   IDs canoniques distincts entre kinds sociaux).
3. **Routage via le dispatcher** : les kinds sociaux sont des bras du match
   `Node::receive_peer_event`, donc APRÈS `admit_peer_event`. Aucun chemin de
   réception ne contourne le gate ; un payload social malformé mais signé est une
   violation **attribuable** (`InvalidEvent`), un flood est contenu par la fenêtre
   glissante (`SpamGuard`) puis l'auteur ignoré.
4. **Stockage isolé** : le cache matérialisé `SocialStore` vit dans une base SQLite
   **dédiée** (`NodeConfig.social_db_path`, tables préfixées `social_*`, schéma
   versionné dans `social_meta`) — zéro collision avec la persistance messages
   (`messages`, `meta`) ; l'échec d'ouverture dégrade proprement (nœud sans cache
   social, jamais un crash au démarrage).

## Alternatives évaluées

- **Renommer `AbuseReport` en 16+ et décaler les sociaux en 15..20** — REJETÉE :
  viole la contrainte 1, casse les événements 15 déjà signés/relayés par les nœuds
  Phase 2.7.
- **Multiplexer les payloads sociaux dans un kind unique (ex. 16 avec un sous-type
  dans `tags`)** — REJETÉE : les IDs canoniques de deux événements de kinds
  différents doivent différer (garantie testée) et le filtrage par kind côté
  gossip/réception serait perdu ; six codes explicites restent auto-documentés.
- **Contournement du gate (handler social direct depuis la couche réseau)** —
  REJETÉE : exposerait un canal de flood non compté ; contraire à la sémantique
  T10 « TOUT événement entrant passe par le gate ».

## Conséquences

- Les pairs anciens (≤ 15) rejettent les événements sociaux sans état corrompu ;
  la convergence se fait par mise à jour logicielle (modèle update existant).
- Le prochain kind mesh devra utiliser **22** ; le test de stabilité échouera si un
  conflit est introduit.
- Les six kinds sont testés en fuzzing (`fuzz_target_5` : décodage/validation/roundtrip
  `SocialPost` sans panique).

## Addendum (vérification T13 — durcissements)

- **I1** : le champ `parent_id` de `SocialPost` est retiré du contrat Rust
  (l'imbrication appartient aux commentaires). Un payload wire historique
  portant encore `parent_id` reste accepté — serde ignore les champs inconnus —
  et sa valeur est ignorée ; la colonne `social_posts.parent_id` n'est plus
  alimentée (schéma stable, compat ascendante).
- **H1** : le chemin de réception distingue deux régimes — payload invalide
  (plafond brut, JSON illisible, bornes domaine) = violation attribuable ;
  échec du cache local = écriture best-effort jamais pénalisante. Les
  commentaires orphelins sont bufferisés (`social_orphan_comments`, schéma v2,
  plafond 1024) puis rejoués à l'arrivée du post.
- **M3** : plafonds bruts par kind AVANT tout décodage (512 kio post/commentaire,
  4 kio vote/follow, 16 kio message privé, 8 kio signalement) + bornes de
  domaine (corps de message privé 2 000 car., motif de signalement 500 car.,
  cibles ≤ 128 car., direction ∈ {-1, 1}, clés publiques hex64).
- **M1 (câblage UI)** : les commandes Tauri `social_*` passent par le `Node`
  réel (`AppState.node`) — identité stable et cache SQLite ouverts à
  `node_start` (`onde-social.sqlite3`) ; plus d'états fantômes. La propagation
  mesh est effective pour publications/commentaires (kinds 16/17) ; votes,
  follows, messages, bookmarks et signalements restent locaux au cache
  (émission UI des kinds 18..21 = pas suivant documenté dans le README).
