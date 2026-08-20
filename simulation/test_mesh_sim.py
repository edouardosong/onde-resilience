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
import math
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import simpy
import mesh_sim
from mesh_sim import (
    Message, MeshNetwork, Node, TechType,
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
