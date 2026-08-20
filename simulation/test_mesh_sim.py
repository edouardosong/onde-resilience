"""
Tests L2-01 — Métriques de simulation mesh (delivery_rate, latence, PoW).

Cause racine du bug (delivery_rate_percent = 32953% au cycle 1) :
dans `MeshNetwork.send_message`, `stats.total_messages_delivered` était
incrémenté pour CHAQUE voisin recevant un message (ligne 416 d\'origine),
sans déduplication. Un message diffusé à N voisins comptait N livraisons,
alors que le dénominateur (`total_messages_sent`) compte 1 par message
→ taux = facteur moyen de diffusion >> 100%.

Ces tests encadrent le contrat attendu :
- 1 message = au plus 1 livraison unique (détection par (sender, msg_id));
- delivery_rate ∈ [0, 100]%;
- latence moyenne > 0 (retard de lien modélisé, et non 0.0 par construction);
- comptage PoW symétrique (success + fail = nombre de messages PoW-gatés).
"""
import json
import math
import random
import shutil
import sys
import os
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import simpy
import mesh_sim
from mesh_sim import (
    Message, MeshNetwork, Node, PoWValidator, TechType, TrafficGenerator,
)


# ----------------------------------------------------------------------------
# Helpers
# ----------------------------------------------------------------------------

def make_network(width_km=1.0, height_km=1.0):
    """Crée un petit réseau déterministe (positions posées à la main)."""
    env = simpy.Environment()
    return env, MeshNetwork(env, width_km=width_km, height_km=height_km)


def place(net, node_id, tech, x, y):
    """Ajoute un nœud à position/technologie contrôlées (pas de random)."""
    node = Node(node_id=node_id, is_bridge=False, tech=tech, x=x, y=y)
    net.nodes[node_id] = node
    net.yggdrasil.assign_address(node_id)
    return node


def advance(env, seconds):
    """Avance l\'horloge de simulation de `seconds` (deterministe)."""
    def _wait():
        yield env.timeout(seconds)
    env.process(_wait())
    env.run()


def make_broadcast(net, sender_id, msg_id="msg-1", size=100, ttl=5):
    """Message de diffusion sans gate PoW (type ai_query)."""
    return Message(
        msg_id=msg_id,
        sender_id=sender_id,
        msg_type="ai_query",
        payload_size_bytes=size,
        ttl=ttl,
        timestamp=env_now_0,
    )


# Le Message ne porte pas d\'horloge ici : on fixe timestamp=0 (création à t=0)
env_now_0 = 0.0


# ----------------------------------------------------------------------------
# 1. Bug principal : 1 message → 2 voisins → 1 livraison UNIQUE
# ----------------------------------------------------------------------------

def test_one_message_two_neighbors_one_unique_delivery():
    """Un message diffusé à 2 voisins dans la portée doit compter 1 livraison."""
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)   # expéditeur
    place(net, 1, TechType.WIFI_AWARE, 550, 500)   # voisin à 50 m (portée 200 m)
    place(net, 2, TechType.WIFI_AWARE, 500, 550)   # voisin à 50 m

    msg = make_broadcast(net, 0)
    assert net.send_message(0, msg) is True

    assert net.stats.total_messages_sent == 1
    # LE BUG (avant fix) : livré == 2 (une par voisin) → taux 200%
    assert net.stats.total_messages_delivered == 1, (
        f"attendu 1 livraison unique, obtenu {net.stats.total_messages_delivered} "
        "(un message remis à N voisins doit compter 1 livraison, pas N)"
    )


def test_delivery_rate_capped_at_100_percent():
    """Le taux de délivrance doit rester dans [0, 100]%."""
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)
    place(net, 1, TechType.WIFI_AWARE, 550, 500)
    place(net, 2, TechType.WIFI_AWARE, 500, 550)

    for i in range(3):
        net.send_message(0, make_broadcast(net, 0, msg_id=f"msg-{i}"))

    s = net.stats
    rate = (s.total_messages_delivered / max(1, s.total_messages_sent)) * 100
    assert 0.0 <= rate <= 100.0, (
        f"taux de délivrance {rate:.1f}% hors bornes "
        f"(livrés={s.total_messages_delivered}, envoyés={s.total_messages_sent})"
    )


# ----------------------------------------------------------------------------
# 2. Livraison DTN (rencontre) : doit compter, sans double-comptage
# ----------------------------------------------------------------------------

def test_dtn_forward_delivers_to_new_node():
    """Un message transmis à un nœud via rencontre DTN = 1 livraison.

    Avant fix : `forward_opportunity` ne comptait AUCUNE livraison → 0.
    """
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 100, 100)    # expéditeur, seul dans sa portée
    place(net, 3, TechType.WIFI_AWARE, 900, 900)    # distant (hors portée directe)

    msg = make_broadcast(net, 0)
    net.send_message(0, msg)
    assert net.stats.total_messages_delivered == 0   # personne dans la portée à t=0

    # La rencontre A↔D plus tard = première livraison du message
    advance(env, 300.0)
    net.dtn_router.forward_opportunity(0, 3, net.stats)

    assert net.stats.total_messages_delivered == 1, (
        f"la rencontre DTN doit compter 1 livraison, obtenu "
        f"{net.stats.total_messages_delivered}"
    )
    # Latence DTN = création (t=0) → première livraison (t=300)
    avg = sum(net.stats.delivery_latency_samples) / max(
        1, len(net.stats.delivery_latency_samples))
    assert avg > 0.0, f"latence moyenne attendue > 0, obtenu {avg}"


def test_dtn_forward_after_direct_delivery_not_double_counted():
    """Livraison directe + forwarding ultérieur du même message = 1 au total."""
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)   # expéditeur
    place(net, 1, TechType.WIFI_AWARE, 550, 500)   # voisin (livraison directe + copie stockée)
    place(net, 2, TechType.WIFI_AWARE, 450, 500)   # voisin (2e livraison directe, doublon)
    place(net, 3, TechType.WIFI_AWARE, 950, 950)   # distant, rencontre plus tard

    msg = make_broadcast(net, 0)
    net.send_message(0, msg)

    # La copie chez le voisin 1 est retransmise lors d\'une rencontre avec 3 :
    # ce n\'est PAS une nouvelle livraison du message (déjà délivré).
    advance(env, 60.0)
    net.dtn_router.forward_opportunity(1, 3, net.stats)

    assert net.stats.total_messages_delivered == 1, (
        f"le même message ne doit compter qu\'une livraison, obtenu "
        f"{net.stats.total_messages_delivered}"
    )


# ----------------------------------------------------------------------------
# 3. Latence moyenne : doit être > 0 (retard de lien modélisé)
# ----------------------------------------------------------------------------

def test_average_latency_positive():
    """Latence moyenne = 0.0 était un artifact : le message était créé ET
    délivré au même tick, sans aucun retard de lien modélisé."""
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)
    place(net, 1, TechType.WIFI_AWARE, 550, 500)

    msg = make_broadcast(net, 0, size=1000)   # 1000 o → ~0.16 ms sur 50 Mbps + 10 ms base
    net.send_message(0, msg)

    assert len(net.stats.delivery_latency_samples) == 1
    avg = sum(net.stats.delivery_latency_samples) / len(
        net.stats.delivery_latency_samples)
    assert avg > 0.0, (
        f"latence moyenne nulle : aucun retard de lien modélisé (obtenu {avg})"
    )
    # WIFI_AWARE : latence de base 10 ms + 1000 o*8 / 50e6 bps = 10 ms + 0.16 ms
    assert 0.010 < avg < 0.011, f"latence attendue ~10.16 ms, obtenu {avg:.6f} s"


# ----------------------------------------------------------------------------
# 4. PoW : comptage symétrique (pas de bug de comptage — gardes-fous)
# ----------------------------------------------------------------------------

def test_pow_counters_symmetric():
    """Chaque message gaté par PoW compte exactement 1 succès OU 1 échec."""
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)
    place(net, 1, TechType.WIFI_AWARE, 550, 500)

    n = 10
    for i in range(n):
        msg = Message(
            msg_id=f"alert-{i}",
            sender_id=0,
            msg_type="alert",
            payload_size_bytes=100,
            ttl=5,
            timestamp=0.0,
        )
        net.send_message(0, msg)

    s = net.stats
    assert s.total_messages_sent == n
    assert s.pow_success + s.pow_fail == n, (
        f"comptage PoW asymétrique : success={s.pow_success}, fail={s.pow_fail} "
        f"pour {n} messages"
    )


def test_pow_failure_blocks_delivery():
    """Un PoW échoué bloque la délivrance (message rejeté)."""
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)
    place(net, 1, TechType.WIFI_AWARE, 550, 500)

    # Difficulté 8 : ~4.5e-6 de chance de succès sur 10 000 essais (déterministe ici)
    net.pow_validator.difficulty = 8
    msg = Message(
        msg_id="alert-pow-fail",
        sender_id=0,
        msg_type="alert",
        payload_size_bytes=100,
        ttl=5,
        timestamp=0.0,
    )
    # Vérifie la prémisse (le calcul est déterministe pour cette entrée)
    assert net.pow_validator.compute_pow(msg, 0.0) is False

    assert net.send_message(0, msg) is False
    assert net.stats.pow_fail == 1
    assert net.stats.total_messages_delivered == 0


# ----------------------------------------------------------------------------
# 4b. L2-09 : cap d'essais PoW = constante module-level MAX_ATTEMPTS
#      (comportement borné, documenté — pas un littéral isolé)
# ----------------------------------------------------------------------------

def test_pow_max_attempts_constant_and_behavior():
    """L2-09 : le cap d'essais PoW est la constante module-level MAX_ATTEMPTS
    (valeur de référence 10000, inchangée), et compute_pow() l'honore :
    avec un target inatteignable (difficulté 8 = 32 bits de zéro), le
    calcul échoue et consomme EXACTEMENT MAX_ATTEMPTS essais — compteur
    `total_attempts` observable sur un validateur frais. Le message échoue
    et n'est pas délivré ; pas de boucle ouverte.

    Déterministe : P(succès/message) = 1-(1-2^(-32))^10000 ≈ 2.3e-6,
    donc l'échec est quasi-certain et reproductible pour cette entrée.
    """
    assert mesh_sim.MAX_ATTEMPTS == 10000, (
        f"MAX_ATTEMPTS = {mesh_sim.MAX_ATTEMPTS}, attendu 10000 : "
        "le cap d'essais PoW est un contrat de comportement (L2-09), "
        "pas un détail interne"
    )

    v = PoWValidator(difficulty=8, adaptive=False)
    msg = Message(
        msg_id="alert-pow-cap",
        sender_id=0,
        msg_type="alert",
        payload_size_bytes=100,
        ttl=5,
        timestamp=0.0,
    )
    assert v.compute_pow(msg, 0.0) is False, (
        "prémisse : target d=8 inatteignable sur 10000 essais (P≈2.3e-6)"
    )
    # Le compteur d'essais du validateur frais prouve que la boucle est
    # bornée par MAX_ATTEMPTS (et non par un littéral déconnecté d'elle) :
    assert v.total_attempts == mesh_sim.MAX_ATTEMPTS, (
        f"compute_pow a consommé {v.total_attempts} essais, attendu "
        f"exactement {mesh_sim.MAX_ATTEMPTS} (cap borné : au-delà, le "
        f"message échoue et n'est pas délivré)"
    )


# ----------------------------------------------------------------------------
# 5. L2-08 : msg_id unique par message émis (2 messages même sender même
#    tick = 2 livraisons uniques, pas 1)
#
# Cause racine : TrafficGenerator générait msg_id = md5(f"{sender}:{env.now}")
# [:12] — deux messages DISTINCTS du même expéditeur émis au même tick de
# simulation (même env.now) produisaient la MÊME clé de dédup (sender_id,
# msg_id) → le fix L2-01 (register_unique_delivery) les comptait comme
# UN SEUL message → sous-comptage des livraisons.
#
# Contrat attendu : l'identité d'un message émis est unique PAR ÉMISSION
# (séquenceur par expéditeur), quelle que soit l'heure de simulation.
# ----------------------------------------------------------------------------

def test_same_sender_same_tick_two_unique_deliveries():
    """L2-08 : 2+ messages du même expéditeur, même tick → chacun compte 1
    livraison unique (pas de dédup entre messages distincts).

    Dispositif : un SEUL nœud mobile (expéditeur) + 1 pont à portée.
    1 tick de `generate_alerts` = randint(5, 50) messages, tous du même
    expéditeur, tous au même env.now → collision de msg_id GARANTIE
    avant fix (tous dédupliqués en 1 livraison), 0 collision après fix.
    """
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)          # expéditeur unique
    net.nodes[1000] = Node(                                # pont à 50 m (portée 200 m)
        node_id=1000, is_bridge=True, tech=TechType.ETHERNET, x=550, y=500,
    )
    # PoW déterministe ici : difficulté 2 → P(échec/msg) ≈ e^-39 ≈ 0,
    # et le comptage ci-dessous intègre pow_fail des deux côtés (invariant exact).
    net.pow_validator.difficulty = 2
    net.pow_validator.adaptive = False

    random.seed(42)
    traffic = TrafficGenerator(env, net)
    env.process(traffic.generate_alerts(interval=5.0, max_nodes_alert=50))
    env.run(until=6.0)   # un seul tick d'émission (t=5.0)

    s = net.stats
    assert s.total_messages_sent >= 5, f"prémisse : ≥5 messages attendus, obtenu {s.total_messages_sent}"
    # LE BUG (avant fix) : delivered == 1 (N messages distincts dédupliqués
    # par le msg_id identique md5("0:<tick>") → comptés comme 1 message).
    assert s.total_messages_delivered >= 2, (
        f"attendu ≥2 livraisons uniques pour {s.total_messages_sent} messages "
        f"distincts du même expéditeur au même tick, obtenu "
        f"{s.total_messages_delivered} (sous-comptage par msg_id non unique)"
    )
    # Invariant exact : chaque message envoyé (passé le PoW) compte
    # EXACTEMENT 1 livraison — ni sous-comptage (L2-08) ni double-comptage (L2-01).
    assert s.total_messages_delivered == s.total_messages_sent - s.pow_fail, (
        f"livrées={s.total_messages_delivered} ≠ envoyées({s.total_messages_sent}) "
        f"- pow_fail({s.pow_fail}) : un message distinct doit compter 1 livraison"
    )


def test_same_sender_same_tick_msg_ids_are_unique():
    """L2-08 : les msg_id émis par le même expéditeur au même tick sont distincts.

    Le buffer DTN de l'expéditeur contient tous les messages émis au tick :
    avant fix, ils partagent TOUS le même msg_id ; après fix, tous distincts.
    """
    env, net = make_network()
    place(net, 0, TechType.WIFI_AWARE, 500, 500)
    net.nodes[1000] = Node(
        node_id=1000, is_bridge=True, tech=TechType.ETHERNET, x=550, y=500,
    )
    net.pow_validator.difficulty = 2
    net.pow_validator.adaptive = False

    random.seed(42)
    traffic = TrafficGenerator(env, net)
    env.process(traffic.generate_alerts(interval=5.0, max_nodes_alert=50))
    env.run(until=6.0)

    emitted = list(net.dtn_router.buffers[0])
    assert len(emitted) >= 2, f"prémisse : ≥2 messages émis, obtenu {len(emitted)}"
    ids = [m.msg_id for m in emitted]
    dup = sorted({i for i in ids if ids.count(i) > 1})
    assert len(ids) == len(set(ids)), (
        f"msg_id non unique pour {len(ids)} messages distincts émis au même tick "
        f"par le même expéditeur : {len(dup)} id dupliqué(s) {dup[:3]}… "
        "(l'identité doit être unique par message émis, pas par (expéditeur, tick))"
    )


# ----------------------------------------------------------------------------
# 11. L2-11 : tx_id ZK unique par émission (même anti-pattern que l'ancien msg_id)
# ----------------------------------------------------------------------------

def test_same_sender_same_tick_tx_ids_are_unique():
    """L2-11 : 2 transactions du même (sender, receiver, amount) au même tick
    doivent porter 2 tx_id DISTINCTS.

    Cause racine (même anti-pattern que l'ancien msg_id, résolu en L2-08) :
    `tx_id = md5(f"{sender}:{receiver}:{amount}:{env.now}")[:12]` — l'horloge
    de simulation `env.now` est la seule composante variable distinguant deux
    transactions identiques créées au même tick → même tx_id. C'est une
    LATENCE : aujourd'hui aucun consommateur ne dédup par tx_id, mais dès que
    ce sera le cas (comme la dédup L2-01/L2-08 sur (sender, msg_id)), le bug
    ressurgira sur les transactions (2 tx distinctes = 1 tx comptée).

    Contrat attendu : l'identité d'une transaction est unique PAR ÉMISSION
    (séquenceur par expéditeur, déterministe sous seed), pas par
    (sender, receiver, amount, tick). Le format reste un digest hex de
    12 caractères (compatibilité consommateurs).
    """
    env, net = make_network()
    # Deux transactions distinctes, mêmes (sender, receiver, amount), même
    # tick (env.now = 0.0, horloge non avancée entre les deux créations).
    tx1 = net.zk_engine.create_transaction(0, 1, 50.0)
    tx2 = net.zk_engine.create_transaction(0, 1, 50.0)

    # AVANT fix (rouge) : tx1["tx_id"] == tx2["tx_id"] == md5("0:1:50.0:0.0")[:12]
    assert tx1["tx_id"] != tx2["tx_id"], (
        f"2 transactions distinctes (sender=0, receiver=1, amount=50.0, même tick) "
        f"partagent le tx_id {tx1['tx_id']!r} : l'identité doit être unique par "
        f"émission, pas par (sender, receiver, amount, tick) — même anti-pattern "
        f"que l'ancien msg_id (L2-08)"
    )
    # Compatibilité : format inchangé = digest hexadécimal 12 caractères.
    for tx in (tx1, tx2):
        assert len(tx["tx_id"]) == 12 and all(c in "0123456789abcdef" for c in tx["tx_id"]), (
            f"le format du tx_id doit rester un digest hex de 12 caractères, "
            f"obtenu {tx['tx_id']!r}"
        )
    # Comportement d'émission inchangé : mêmes champs métier, même timestamp.
    assert (tx1["sender"], tx1["receiver"], tx1["amount"], tx1["timestamp"]) == \
           (tx2["sender"], tx2["receiver"], tx2["amount"], tx2["timestamp"])


# ----------------------------------------------------------------------------
# 12. L2-12 : seed = paramètre explicite et documenté de run_simulation()
#
# Constat (checker L2-08) : la reproductibilité des runs dépendait du fait
# que l'APPELANT appelle random.seed(42) avant run_simulation(). Sans cette
# convention externe, deux runs consécutifs étaient DIFFÉRENTS (le 2e run
# hérite de l'état du PRNG global consommé par le 1er) — la reproductibilité
# tenait à un usage, pas à l'API.
#
# Contrat : `seed` est un paramètre explicite de l'API (défaut 42 = la valeur
# utilisée partout dans les tests/preuves), et run_simulation() applique
# random.seed(seed) elle-même :
# - 2 runs avec seed=42  → rapports byte-identiques (hors wall-time) ;
# - 1 run avec seed=7    → stats DIFFÉRENTES (la seed agit réellement).
# ----------------------------------------------------------------------------

def _report_without_walltime(report: dict) -> dict:
    """Copie profonde du rapport SANS les champs wall-time (``real_time_sec``)
    — la seule partie du rapport non déterministe (mesure réelle d'exécution)."""
    r = json.loads(json.dumps(report))
    r["simulation_config"].pop("real_time_sec", None)
    return r


def test_run_simulation_seed_determinism():
    """L2-12 : déterminisme garanti par l'API (paramètre seed), pas par
    convention externe de l'appelant.

    AVANT fix : ROUGE — `run_simulation()` n'a pas de paramètre `seed`
    (TypeError), et sans random.seed() de l'appelant, 2 runs consécutifs
    diffèrent (état global du PRNG). APRES fix : VERT.
    """
    # Configuration réduite (le contrat testé est la DETERMINATION, pas
    # l'échelle) — un run complet (10k nœuds / 1 h simulée) coûte ~3 min.
    cfg = dict(
        sim_duration=120.0,
        mobile_count=150,
        bridge_count=15,
        area_km=2.0,
        report_interval=30.0,
    )

    # run_simulation() sauvegarde son rapport en chemin RELATIF
    # (onde/simulation/results/...) : on l'isole dans un répertoire
    # temporaire pour ne pas écraser les artefacts du dépôt.
    cwd = os.getcwd()
    tmp = tempfile.mkdtemp(prefix="onde_seed_test_")
    os.chdir(tmp)
    try:
        r42_a = _report_without_walltime(mesh_sim.run_simulation(seed=42, **cfg))
        r42_b = _report_without_walltime(mesh_sim.run_simulation(seed=42, **cfg))
        r7 = _report_without_walltime(mesh_sim.run_simulation(seed=7, **cfg))
    finally:
        os.chdir(cwd)
        shutil.rmtree(tmp, ignore_errors=True)

    # 1) Même seed → rapport byte-identique (hors wall-time).
    assert r42_a == r42_b, (
        "2 runs avec seed=42 doivent être byte-identiques (hors wall-time) ; "
        "déterminisme attendu garanti par l'API (L2-12), pas par l'état "
        "global du PRNG"
    )

    # 2) Seed différente → stats DIFFÉRENTES (la seed agit réellement sur
    #    le PRNG global : positions des nœuds, trafic, PoW, rencontres).
    assert r7["network_stats"] != r42_a["network_stats"], (
        f"seed=7 doit produire des stats différentes de seed=42 "
        f"(obtenu identique : {r7['network_stats']})"
    )



# ----------------------------------------------------------------------------
# 13. L2-10 / RECO checker L2-04 : régression `encounter_opportunity`
#     (NAIVE vs BUCKETÉ — même ensemble de paires, bord dist=S, hors-portée)
#
# Objet : LA recherche des paires de rencontre utilise indifféremment
# `_encounter_pairs_naive` (double-boucle O(m²), référence exacte) ou
# `_encounter_pairs_bucketed` (bucketing spatial EXACT, O(m·k)). Un bug de
# régression dans l'un des deux chemins (le bucketing surtout, plus complexe)
# changerait l'ensemble des rencontres et donc la consommation du PRNG et les
# mutations DTN (forward_opportunity) — sans casser forcément un test existant.
# Ce test encadre le contrat : même ensemble de paires, même traitement du
# bord `dist = S` (portée exacte), zéro faux positif hors-portée.
# Déterministe : positions explicites (aucun PRNG) + seed fixe pour le test
# end-to-end. Rapide (< 5 s).
# ----------------------------------------------------------------------------

# Cas contrôlé L2-10 : 6 nœuds, positions posées à la main, portées connues.
# S = max range de l'échantillon = 200 m (WIFI_AWARE). Comprend :
#   - un bord EXACT dist == S   (A↔B : 200.0 m = portée WIFI 200 m) ;
#   - un bord JUSTE AU-DESSUS    (A↔C : 200.5 m > S) → hors-portée ;
#   - une portée ASYMÉTRIQUE      (BLE 50 m vs WIFI 200 m) ;
#   - un nœud LOINTAIN            (F à 14 km) → aucun faux positif.
ENCOUNTER_POSITIONS = [
    (0, 0.0,    0.0,    TechType.WIFI_AWARE),  # A — expéditeur (portée 200)
    (1, 200.0,  0.0,    TechType.WIFI_AWARE),  # B — dist = 200 = S (bord)
    (2, 200.5,  0.0,    TechType.WIFI_AWARE),  # C — dist = 200.5 > S (hors)
    (3, 0.0,    150.0,  TechType.WIFI_AWARE),  # D — dist(A)=150 ≤ 200
    (4, 0.0,    40.0,   TechType.BLE),         # E — range 50 (asymétrique)
    (5, 10000.0, 10000.0, TechType.WIFI_AWARE),# F — très loin (aucune paire)
]

# Ensemble exact attendu (calculé à la main sur les portées connues) :
# (A,B)=200 ✓ · (A,C)=200.5 ✗ · (A,D)=150 ✓ · (A,E)=40 ≤ max(200,50) ✓ ·
# (B,C)=0.5 ✓ · (B,D)=250 ✗ · (B,E)≈203.96>200 ✗ · (C,D)≈250.4 ✗ ·
# (C,E)≈204.45 ✗ · (D,E)=110 ✓ · toute paire avec F : ✗.
ENCOUNTER_EXPECTED = {(0, 1), (0, 3), (0, 4), (1, 2), (3, 4)}


def test_encounter_naive_vs_bucketed_same_pairs():
    """L2-10 : les chemins NAIVE et BUCKETÉ produisent le même ENSEMBLE de
    paires (ordre indifférent), et cet ensemble est exactement celui attendu
    sur le cas contrôlé (portées connues)."""
    naive = set(MeshNetwork._encounter_pairs_naive(ENCOUNTER_POSITIONS))
    bucketed = set(MeshNetwork._encounter_pairs_bucketed(ENCOUNTER_POSITIONS))

    # 1) même ensemble de paires opportunes (ordre indifférent)
    assert naive == bucketed, (
        "chemins NAIVE/BUCKETÉ divergents\n"
        f"  naive   = {sorted(naive)}\n"
        f"  bucketed= {sorted(bucketed)}"
    )
    # 2) cet ensemble est exactement les paires attendues (référence manuelle)
    assert naive == ENCOUNTER_EXPECTED, (
        "l'ensemble NAIVE/BUCKETÉ ne correspond pas au cas contrôlé attendu :\n"
        f"  obtenu   = {sorted(naive)}\n"
        f"  attendu  = {sorted(ENCOUNTER_EXPECTED)}"
    )


def test_encounter_boundary_dist_equal_range_identical():
    """L2-10 : le bord `dist = S` (portée EXACTE) est traité de façon
    IDENTIQUE entre NAIVE et BUCKETÉ.

    Code : les deux chemins utilisent la même inégalité non stricte
    ``dist_sq <= max_range²`` → l'inclusion/exclusion du bord est identique.
    Ici la paire A↔B (dist exactement 200 m = portée WIFI 200 m) doit être
    INCLUSE par les deux, et A↔C (200.5 m, juste au-dessus) EXCLUE par les
    deux. Toute asymétrie de bord entre les chemins = régression.
    """
    naive = set(MeshNetwork._encounter_pairs_naive(ENCOUNTER_POSITIONS))
    bucketed = set(MeshNetwork._encounter_pairs_bucketed(ENCOUNTER_POSITIONS))

    boundary_pair = (0, 1)   # A↔B : dist = 200.0 = S
    above_pair = (0, 2)      # A↔C : dist = 200.5 > S

    assert boundary_pair in naive and boundary_pair in bucketed, (
        f"la paire au bord exact dist == S ({boundary_pair}) doit être INCLUSE "
        f"par les deux chemins (naive={boundary_pair in naive}, "
        f"bucketed={boundary_pair in bucketed}) — inégalité non stricte"
    )
    assert above_pair not in naive and above_pair not in bucketed, (
        f"la paire légèrement au-dessus du bord ({above_pair}, dist=200.5 > S) "
        f"doit être EXCLUE par les deux chemins — cohérence d'exclusion du bord"
    )


def test_encounter_no_out_of_range_false_positive():
    """L2-10 : AUCUN faux positif hors-portée dans l'un OU l'autre chemin.

    Pour chaque paire rapportée (dans quelque chemin que ce soit), la
    distance réelle doit être ≤ max(range(a), range(b)) — sinon le chemin
    invente une rencontre hors de portée (faux positif), biaisant la
    consommation du PRNG et les mutations DTN.
    """
    from mesh_sim import TECH_PROFILES
    for positions, name in (
        (ENCOUNTER_POSITIONS, "cas contrôlé"),
    ):
        for pairs, pname in (
            (MeshNetwork._encounter_pairs_naive(positions), "naive"),
            (MeshNetwork._encounter_pairs_bucketed(positions), "bucketed"),
        ):
            for (a, b) in pairs:
                a_row = next(p for p in positions if p[0] == a)
                b_row = next(p for p in positions if p[0] == b)
                dist = math.hypot(a_row[1] - b_row[1], a_row[2] - b_row[2])
                max_range = max(TECH_PROFILES[a_row[3]].range_m,
                                TECH_PROFILES[b_row[3]].range_m)
                assert dist <= max_range, (
                    f"FAUX POSITIF hors-portée {pname} : {a}-{b} dist={dist:.1f}m "
                    f"> portée {max_range}m (contredit la définition d'une rencontre)"
                )
            # chaîne fermée : toutes les paires hors portée sont absentes
            all_ids = [p[0] for p in positions]
            for i, a in enumerate(all_ids):
                for b in all_ids[i + 1:]:
                    a_row = next(p for p in positions if p[0] == a)
                    b_row = next(p for p in positions if p[0] == b)
                    dist = math.hypot(a_row[1] - b_row[1], a_row[2] - b_row[2])
                    max_range = max(TECH_PROFILES[a_row[3]].range_m,
                                    TECH_PROFILES[b_row[3]].range_m)
                    in_pairs = (a, b) in pairs
                    if dist <= max_range:
                        assert in_pairs, (
                            f"rencontre manquée (faux négatif) {pname} : "
                            f"{a}-{b} dist={dist:.1f} ≤ portée {max_range}m"
                        )
                    else:
                        assert not in_pairs, (
                            f"faux positif hors-portée {pname} : "
                            f"{a}-{b} dist={dist:.1f} > portée {max_range}m"
                        )
            break  # un seul cas de contrôlé suffit pour la chaîne fermée


def test_encounter_dispatch_matches_naive_and_is_deterministic():
    """L2-10 : `encounter_opportunity()` end-to-end (sélecteur + forward DTN)
    est déterministe (seed fixe) et produit le même ensemble de rencontres
    sur un domaine assez grand pour que `_encounter_pairs` choisisse le
    chemin BUCKETÉ (grille ≥ 18 cellules), comparé à la référence NAIVE.

    La recherche (échantillonnage + paires exactes) est découplée du forward :
    on compare l'ensemble des paires décidé par le sélecteur à celui de la
    référence NAIVE appliquée aux mêmes positions échantillonnées.
    """
    env = simpy.Environment()
    net = MeshNetwork(env, width_km=2.0, height_km=2.0)  # 2000×2000 m
    # Positions explicites (déterministe, pas de PRNG) sur un domaine 2000×2000 :
    # avec S = 200 m (WIFI), n_cells=(2000/200)²=100 ≥ 18 → chemin BUCKETÉ choisi.
    for nid, (x, y) in {
        0: (0, 0), 1: (200, 0), 2: (200, 200), 3: (0, 200),
        4: (400, 0), 5: (400, 400), 6: (800, 800), 7: (1200, 300),
    }.items():
        net.nodes[nid] = Node(node_id=nid, is_bridge=False,
                              tech=TechType.WIFI_AWARE, x=x, y=y)
        net.yggdrasil.assign_address(nid)

    sample_ids = sorted(net.nodes.keys())
    positions = [(nid, net.nodes[nid].x, net.nodes[nid].y, net.nodes[nid].tech)
                 for nid in sample_ids]

    # Le sélecteur doit réellement passer par le BUCKETING (preuve de chemin).
    S = max(mesh_sim.TECH_PROFILES[p[3]].range_m for p in positions)
    n_cells = (net.width_m / S) * (net.height_m / S)
    assert n_cells >= 18, f"prémisse : bucketing attendu (n_cells={n_cells}), non atteint"

    decided = set(net._encounter_pairs(positions))           # chemin dispatché
    reference = set(MeshNetwork._encounter_pairs_naive(positions))  # référence exacte

    assert decided == reference, (
        "le sélecteur dispatché ne reproduit pas la référence NAIVE :\n"
        f"  dispatch = {sorted(decided)}\n"
        f"  naive    = {sorted(reference)}"
    )

    # Déterminisme : 2 exécutions de `encounter_opportunity` avec seed fixe
    # comptent le même nombre de rencontres et ne mutent rien d'incohérent.
    import random as _random
    _random.seed(1234)
    first = net.encounter_opportunity()
    _random.seed(1234)
    second = net.encounter_opportunity()
    assert first == second, (
        f"encounter_opportunity() non déterministe sous seed fixe : "
        f"{first} != {second}"
    )


# ----------------------------------------------------------------------------
# 15. L2-14 / ADR-001 : rencontre en bucketing multi-tier (option b)
#     `_encounter_pairs_tiered` == `_encounter_pairs_naive` (même ensemble ET
#     même ordre). Le dispatch nominal est désormais le chemin multi-tier, qui
#     remplace la grille unique S=max range (L2-04) dégénérée en O(m²) dès qu'un
#     pont (ETHERNET 999999 m) est échantillonné (cf. docs/adr/ADR-001-encounter-perf.md).
#     Ces tests verrouillent l'équivalence sémantique exacte du nouveau chemin.
# ----------------------------------------------------------------------------

# Technologies à répartir (mobiles WIFI/BLE/LORA + pont ETHERNET).
_TIER_TECHS = [TechType.WIFI_AWARE, TechType.BLE, TechType.LORA, TechType.ETHERNET]
_TIER_TECHS_NO_BRIDGE = [TechType.WIFI_AWARE, TechType.BLE, TechType.LORA]
# Domaines de test (m) — inclut des coordonnées NÉGATIVES pour vérifier le hash
# de cellule sur toute la droite réelle.
_TIER_DOMAINS = [100, 500, 2000, 5000, 100000]


def _generate_tier_positions(seed, n, domain, techs):
    """Positions déterministes (random local, aucune fuite d'état global)."""
    rng = random.Random(seed)
    return [(i,
             rng.uniform(-domain, domain),
             rng.uniform(-domain, domain),
             rng.choice(techs))
            for i in range(n)]


def test_encounter_tiered_matches_naive_sequence():
    """L2-14 : `_encounter_pairs_tiered` reproduit EXACTEMENT la référence
    `_encounter_pairs_naive` — même ENSEMBLE de paires ET même ORDRE (i, j) —
    sur une séquence déterministe de configurations mixtes (4 tiers de portée,
    coords ±, domaines variés). C'est la garantie de non-régression de l'option
    b (ADR-001) : le chemin multi-tier est interchangeable avec le double-boucle
    jusqu'à l'ordre d'émission (déterminant pour la chaîne de mutations DTN et
    la consommation du PRNG)."""
    for seed in range(60):
        rng = random.Random(seed)
        n = rng.randint(2, 40)
        domain = rng.choice(_TIER_DOMAINS)
        techs = rng.choice([_TIER_TECHS, _TIER_TECHS_NO_BRIDGE])
        pos = _generate_tier_positions(seed, n, domain, techs)
        naive = MeshNetwork._encounter_pairs_naive(pos)
        tiered = MeshNetwork._encounter_pairs_tiered(pos)
        assert tiered == naive, (
            "ordre non reproductible (L2-14) : tiered != naive\n"
            f"  seed={seed} n={n} domain={domain}\n"
            f"  naive ={naive}\n"
            f"  tiered={tiered}"
        )
        assert set(tiered) == set(naive), (
            f"ensemble divergent (L2-14) à seed={seed} : "
            f"{sorted(set(tiered) ^ set(naive))}"
        )


def test_encounter_tiered_bridge_present_matches_naive():
    """L2-14 : cas contrôlé avec un PONT (ETHERNET) dans un petit domaine —
    c'est EXACTEMENT la configuration où l'ancien bucketing L2-04 (S = max range
    999999 m → ~1 cellule → n_cells < 18) retombait sur le double-boucle O(m²).
    Le chemin multi-tier doit produire la même sortie que la référence.

    Positions explicites (déterministe, aucun PRNG) :
      A(10,10,WIFI 200) B(30,10,WIFI) C(5,5,BLE 50) D(40,40,LORA 5000)
      E(60,60,ETHERNET bridge 999999) F(200,200,WIFI)
    Le bridge E voit TOUT (portée ≫ domaine) → A,B,C,D,F sont tous rencontrés.
    Reste mobile-mobile : A-B (20≤200✓), A-C(≈7.07≤200✓), B-C(≈25.5≤200✓),
    A-D(≈42.4≤5000✓), etc. (le filtre exact est celui de la référence)."""
    pos = [
        (0, 10.0, 10.0, TechType.WIFI_AWARE),     # A
        (1, 30.0, 10.0, TechType.WIFI_AWARE),     # B
        (2, 5.0, 5.0, TechType.BLE),              # C
        (3, 40.0, 40.0, TechType.LORA),           # D
        (4, 60.0, 60.0, TechType.ETHERNET),       # E bridge
        (5, 200.0, 200.0, TechType.WIFI_AWARE),   # F
    ]
    naive = MeshNetwork._encounter_pairs_naive(pos)
    tiered = MeshNetwork._encounter_pairs_tiered(pos)
    assert tiered == naive, (
        "bridge dans petit domaine (L2-14) : tiered != naive\n"
        f"  naive ={naive}\n  tiered={tiered}"
    )
    # Le bridge E (id 4) est bien en rencontre avec TOUS les autres nœuds :
    # les paires (i, j) sont émis avec i < j, donc le bridge est en 1re position
    # pour les id > 4 (→ (4,5)) et en 2e position pour les id < 4 (→ (0,4)...(3,4)).
    bridge_pairs = {(i, j) for (i, j) in tiered if i == 4 or j == 4}
    expected_bridge = {(i, 4) for i in (0, 1, 2, 3)} | {(4, 5)}
    assert bridge_pairs == expected_bridge, (
        "le bridge (portée 999999 m) doit rencontrer tous les nœuds du domaine : "
        f"got {sorted(bridge_pairs)}, expected {sorted(expected_bridge)}"
    )


def test_encounter_tiered_boundary_dist_equal_and_no_false_positive():
    """L2-14 : le bord `dist = S` (portée EXACTE) est traité identiquement entre
    le chemin multi-tier et la référence NAIVE, et AUCUN faux positif hors-portée
    n'est rapporté — sur l'ensemble des paires de plusieurs configurations.

    Inégalité non stricte `dist_sq <= R*²` partagée : une paire à dist == max portée
    est INCLUSE par les deux chemins ; une paire juste au-dessus est EXCLUE. Tout
    faux positif (paire rapportée alors que dist > R*) = bug de l'énumérateur."""
    from mesh_sim import TECH_PROFILES
    cases = [ENCOUNTER_POSITIONS] + [
        _generate_tier_positions(seed, 25, domain, _TIER_TECHS)
        for seed in range(5) for domain in _TIER_DOMAINS]
    for positions in cases:
        for name, pairs in (
            ("naive",  MeshNetwork._encounter_pairs_naive(positions)),
            ("tiered", MeshNetwork._encounter_pairs_tiered(positions)),
        ):
            all_ids = [p[0] for p in positions]
            for i, a in enumerate(all_ids):
                for b in all_ids[i + 1:]:
                    a_row = next(p for p in positions if p[0] == a)
                    b_row = next(p for p in positions if p[0] == b)
                    dist = math.hypot(a_row[1] - b_row[1], a_row[2] - b_row[2])
                    max_range = max(TECH_PROFILES[a_row[3]].range_m,
                                    TECH_PROFILES[b_row[3]].range_m)
                    present = (a, b) in pairs
                    if dist <= max_range:
                        assert present, (
                            f"rencontre manquée (faux négatif) {name} : "
                            f"{a}-{b} dist={dist:.1f} ≤ portée {max_range}m"
                        )
                    else:
                        assert not present, (
                            f"faux positif hors-portée {name} : "
                            f"{a}-{b} dist={dist:.1f} > portée {max_range}m"
                        )


def test_encounter_dispatch_uses_tiered_equivalent():
    """L2-14 : le dispatch `_encounter_pairs` (chemin nominal = multi-tier)
    reproduit la référence NAIVE — ENSEMBLE et ORDRE compris — sur deux régimes :
    (a) un échantillon varié (WIFI/BLE/LORA, petite grille) et (b) un petit
    régime avec pont (config où l'ANCIEN bucketing à grille unique L2-04
    n_cells < 18 retombait sur le double-boucle O(m²)). Le dispatch n'altère
    jamais le résultat (le choix de l'enumérateur est un facteur constant,
    cf. ADR-001). Le ROUTAGE effectif vers le chemin multi-tier est verrouillé
    séparément par `test_encounter_dispatch_routing_to_tiered` (spy d'appel) :
    ce test-ci compare les SORTIES, l'autre prouve que c'est bien le tiered qui
    est appelé depuis le dispatch. Termine par le déterminisme end-to-end sous
    seed fixe (2 exécutions identiques)."""
    # (a) Échantillon varié (WIFI/BLE/LORA) : le dispatch doit valoir NAIVE.
    big_pos = ([(i, (i % 20) * 200.0, (i // 20) * 200.0, TechType.WIFI_AWARE)
                for i in range(16)] +
               [(16, 10.0, 10.0, TechType.LORA),
                (17, 1000.0, 1000.0, TechType.BLE)])
    env = simpy.Environment()
    net = MeshNetwork(env, width_km=10.0, height_km=10.0)
    for nid, x, y, tech in big_pos:
        net.nodes[nid] = Node(node_id=nid, is_bridge=(tech == TechType.ETHERNET),
                              tech=tech, x=x, y=y)
        net.yggdrasil.assign_address(nid)

    decided = net._encounter_pairs(big_pos)                      # dispatch (tiered)
    reference = MeshNetwork._encounter_pairs_naive(big_pos)      # référence exacte
    assert decided == reference, (
        "le dispatcher (tiered) ne reproduit pas NAIVE :\n"
        f"  dispatch = {decided}\n  naive    = {reference}"
    )

    # (b) Petit régime avec bridge (n_cells<18 sous l'ancienne grille unique) :
    #     le dispatch doit quand même égaler NAIVE (config du test bridge).
    small_pos = [
        (0, 10.0, 10.0, TechType.WIFI_AWARE),
        (1, 30.0, 10.0, TechType.WIFI_AWARE),
        (2, 5.0, 5.0, TechType.BLE),
        (3, 60.0, 60.0, TechType.ETHERNET),
    ]
    decided_s = net._encounter_pairs(small_pos)
    reference_s = MeshNetwork._encounter_pairs_naive(small_pos)
    assert decided_s == reference_s, (
        f"dispatch (petit régime bridge) != naïve : {decided_s} != {reference_s}"
    )

    # (c) Déterminisme sous seed fixe (end-to-end `encounter_opportunity`).
    random.seed(777)
    first = net.encounter_opportunity()
    random.seed(777)
    second = net.encounter_opportunity()
    assert first == second, (
        f"encounter_opportunity() non déterministe (L2-14) : {first} != {second}"
    )


def test_encounter_tiered_bucketed_naive_all_agree():
    """L2-14 : sur un régime où le bucketing L2-04 à grille fine s'applique
    (n_cells ≥ 18, portées étroites seules), les TROIS chemins — multi-tier,
    bucketing unique et double-boucle — produisent la même sortie (set ET ordre).
    Garantit l'interchangeabilité des implémentations sans régression croisée.

    Config : 12 nœuds WIFI/BLE étalés sur 1000×1000 m, S = max range = 5000
    (LORA) → n_cells = (1000/5000)² = 0.04 < 18 ... on force plutôt une grille
    fine : on retire LORA et on utilise S = 200 (WIFI) → n_cells = (1000/200)²=25
    ≥ 18 → le bucketing L2-04 s'applique réellement."""
    pos = [(i, (i % 5) * 180.0, (i // 5) * 180.0, TechType.WIFI_AWARE)
           for i in range(12)]
    pos.append((12, 900.0, 900.0, TechType.BLE))
    S = max(mesh_sim.TECH_PROFILES[p[3]].range_m for p in pos)
    n_cells = (1000.0 / S) * (1000.0 / S)
    assert n_cells >= 18, f"prémisse bucketing fin : n_cells={n_cells}"

    naive = MeshNetwork._encounter_pairs_naive(pos)
    bucketed = MeshNetwork._encounter_pairs_bucketed(pos)
    tiered = MeshNetwork._encounter_pairs_tiered(pos)
    assert naive == bucketed == tiered, (
        "NAIVE / BUCKETÉ / TIERED divergents (L2-14) :\n"
        f"  naive   = {naive}\n"
        f"  bucketed= {bucketed}\n"
        f"  tiered  = {tiered}"
    )


def test_encounter_tiered_exact_boundary_multiples_no_false_negative():
    """L2-14 (revue CALL-A) : RÉGRESSION — faux négatif de l'exactitude FP quand
    une coordonnée est un multiple EXACT de la portée R ET que dist = R exact.

    Contre-exemple du checker : A=(15999984, 0) WIFI (r=200), B=(16999983, 0)
    ETHERNET (r=999999). dist = 900000? Non : |16999983 − 15999984| = 999999 = R*
    exact, et les coords sont des multiples de R* : 15999984 = 16×999999,
    16999983 = 17×999999. Avec le calcul FP fautif `x * (1.0/R)`, on obtient
    `x_a * inv_R` = 15.999999999999998 → floor 15 alors que `x_a / R` = 16.0
    (floor 16) : la cellule était décalée de 2 → (a,b) hors fenêtre 3×3 →
    tiered/bucketed = [] alors que naive = [(0,1)]. Le correctif (division
    DIRECTE `x / R`, `x / S`) doit rendre tiered == bucketed == naive == [(0,1)].

    Axe x ET axe y (rôles inversés), + héritage du fix dans `_encounter_pairs_bucketed`."""
    # Contre-exemple : axe x
    px = [
        (0, 15999984.0, 0.0, TechType.WIFI_AWARE),   # = 16 × 999999
        (1, 16999983.0, 0.0, TechType.ETHERNET),     # = 17 × 999999
    ]
    assert MeshNetwork._encounter_pairs_naive(px) == [(0, 1)], (
        "précondition : la référence NAIVE doit voir la paire (0,1)"
    )
    assert MeshNetwork._encounter_pairs_tiered(px) == [(0, 1)], (
        "FAUX NÉGATIF L2-14 : tiered rate la paire au multiple exact de R (x)"
    )
    assert MeshNetwork._encounter_pairs_bucketed(px) == [(0, 1)], (
        "FAUX NÉGATIF L2-04 : bucketed rate la paire au multiple exact de R (x)"
    )

    # Axe y (rôles inversés : coords portées par y)
    py = [
        (0, 0.0, 15999984.0, TechType.WIFI_AWARE),
        (1, 0.0, 16999983.0, TechType.ETHERNET),
    ]
    assert MeshNetwork._encounter_pairs_tiered(py) == [(0, 1)], (
        "FAUX NÉGATIF L2-14 : tiered rate la paire au multiple exact de R (y)"
    )
    assert MeshNetwork._encounter_pairs_bucketed(py) == [(0, 1)], (
        "FAUX NÉGATIF L2-04 : bucketed rate la paire au multiple exact de R (y)"
    )


def test_encounter_dispatch_routing_to_tiered():
    """L2-14 (revue CALL-A) : verrou de ROUTAGE — le dispatch `_encounter_pairs`
    appelle bien le chemin multi-tier `_encounter_pairs_tiered` (et non la
    référence naive). Sans ce spy, une régression qui ferait appel au naive
    passerait les tests de sortie (le contenu serait identique) : ici on compte
    les APPELS réels via un wrapper compteur, puis on restaure l'original.
    Couvre le dispatch direct ET la route end-to-end `encounter_opportunity`."""
    positions = [
        (0, 10.0, 10.0, TechType.WIFI_AWARE),
        (1, 30.0, 10.0, TechType.WIFI_AWARE),
        (2, 5.0, 5.0, TechType.BLE),
        (3, 40.0, 40.0, TechType.LORA),
        (4, 60.0, 60.0, TechType.ETHERNET),
    ]
    env = simpy.Environment()
    net = MeshNetwork(env, width_km=10.0, height_km=10.0)
    for nid, x, y, tech in positions:
        net.nodes[nid] = Node(node_id=nid, is_bridge=(tech == TechType.ETHERNET),
                              tech=tech, x=x, y=y)
        net.yggdrasil.assign_address(nid)

    orig = MeshNetwork._encounter_pairs_tiered
    calls = []
    # Wrapper compteur (staticmethod pour préserver la signature statique réelle).
    def spy(*args, **kwargs):
        calls.append(1)
        return orig(*args, **kwargs)
    try:
        MeshNetwork._encounter_pairs_tiered = staticmethod(spy)
        # (a) dispatch direct
        net._encounter_pairs(positions)
        # (b) route end-to-end `encounter_opportunity` (qui passe par le dispatch)
        net.encounter_opportunity()
        assert len(calls) >= 2, (
            "ROUTAGE cassé : le dispatch/`encounter_opportunity` n'appelle pas "
            f"_encounter_pairs_tiered (calls={calls})"
        )
    finally:
        # ⚠ Restauration OBLIGATOIRE en staticmethod (sinon `self.` lie la méthode
        # et le dispatch reçoit 2 args). `orig` (accès classe) est la fonction nue.
        MeshNetwork._encounter_pairs_tiered = staticmethod(orig)
