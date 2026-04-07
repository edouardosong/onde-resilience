#!/usr/bin/env python3
"""
ONDE — Réseau de Résilience Citoyen
PHASE 1 — Simulateur de réseau mesh hybride (SimPy)

Simule 500 000 nœuds mobiles + 50 000 ponts desktop
Tests: text gossip, voix Opus compressée, DTN routing, PoW, ZK transactions
"""

import simpy
import random
import math
import hashlib
import time
import json
import os
from collections import defaultdict, deque
from dataclasses import dataclass, field
from enum import IntEnum
from typing import Optional
import logging

logging.basicConfig(level=logging.INFO, format='%(asctime)s [%(levelname)s] %(message)s')
logger = logging.getLogger("onde_sim")

# ==============================================================================
# CONFIGURATION
# ==============================================================================

class TechType(IntEnum):
    """Technologies de communication supportées."""
    WIFI_AWARE = 0      # 200m range, 50 Mbps
    BLE = 1             # 50m range, 2 Mbps
    LORA = 2            # 5000m range, 50 kbps
    ETHERNET = 3        # Wired bridge (desktop only), 1 Gbps
    HYBRID = 4          # Multi-tech node

@dataclass
class TechProfile:
    range_m: float          # Portée en mètres
    bandwidth_bps: float    # Bande passante en bits/sec
    latency_ms: float       # Latence moyenne
    power_consumption_mw: float

TECH_PROFILES = {
    TechType.WIFI_AWARE: TechProfile(200, 50e6, 10, 100),
    TechType.BLE:        TechProfile(50, 2e6, 30, 5),
    TechType.LORA:       TechProfile(5000, 50e3, 100, 50),
    TechType.ETHERNET:   TechProfile(999999, 1e9, 1, 0),
}

@dataclass
class Message:
    msg_id: str
    sender_id: int
    msg_type: str          # "alert", "mutual_aid", "voice", "transaction", "ai_query"
    payload_size_bytes: int
    ttl: int
    hop_count: int = 0
    timestamp: float = 0.0
    destination: Optional[int] = None  # None = broadcast
    pow_hash: str = ""
    encrypted: bool = True

@dataclass
class Node:
    node_id: int
    is_bridge: bool         # Desktop/Ethernet bridge
    tech: TechType
    x: float                # Position (simulation 2D)
    y: float
    velocity: float = 0.0   # m/s (0 pour desktop)
    direction: float = 0.0  # radians
    buffer_capacity: int = 1000  # messages DTN
    battery_percent: float = 100.0
    ai_capable: bool = False  # Desktop = super oracle
    ai_model_size_gb: float = 0.0
    storage_gb: float = 0.0

@dataclass
class SimStats:
    total_messages_sent: int = 0
    total_messages_delivered: int = 0
    total_messages_dropped: int = 0
    total_messages_expired: int = 0
    total_dtn_hops: int = 0
    total_voice_bytes: int = 0
    total_text_bytes: int = 0
    total_tx_processed: int = 0
    pow_success: int = 0
    pow_fail: int = 0
    delivery_latency_samples: list = field(default_factory=list)
    queue_depth_history: list = field(default_factory=list)

# ==============================================================================
# POW (Proof of Work) — Antispam local
# ==============================================================================

class PoWValidator:
    """Preuve de travail CPU pour chaque message public."""
    
    def __init__(self, difficulty: int = 4, adaptive: bool = True):
        self.difficulty = difficulty  # nombre de zéros en préfixe
        self.adaptive = adaptive
        self.total_attempts = 0
    
    def compute_pow(self, msg: Message, env_time: float) -> bool:
        """Simule le calcul PoW. Retourne True si réussi."""
        target = '0' * self.difficulty
        nonce = 0
        max_attempts = 10000  # ~1ms CPU
        
        data = f"{msg.sender_id}:{msg.msg_type}:{msg.msg_id}:{env_time}"
        
        for _ in range(max_attempts):
            self.total_attempts += 1
            attempt = f"{data}:{nonce}".encode()
            h = hashlib.sha256(attempt).hexdigest()
            if h.startswith(target):
                msg.pow_hash = h
                return True
            nonce += 1
        
        return False
    
    def adjust_difficulty(self, network_load: float):
        """Ajuste la difficulté selon la charge réseau."""
        if self.adaptive:
            if network_load > 0.8:
                self.difficulty = min(8, self.difficulty + 1)
            elif network_load < 0.2:
                self.difficulty = max(2, self.difficulty - 1)

# ==============================================================================
# DTN ROUTER — Store and Forward
# ==============================================================================

class DTNRouter:
    """Routage Delay-Tolerant Network avec store-and-forward."""
    
    def __init__(self, env: simpy.Environment):
        self.env = env
        self.buffers: dict[int, deque] = defaultdict(lambda: deque(maxlen=2000))
        self.routing_table: dict[int, list[int]] = {}  # node_id -> known neighbors
        self.forward_count = 0
        self.drop_count = 0
    
    def store_message(self, node_id: int, msg: Message) -> bool:
        """Stocke un message dans le buffer DTN d'un nœud."""
        buf = self.buffers[node_id]
        if len(buf) < 2000:
            buf.append(msg)
            return True
        # Buffer plein — évince le plus ancien (FIFO simplifié)
        buf.popleft()
        buf.append(msg)
        self.drop_count += 1
        return True
    
    def forward_opportunity(self, node_a: int, node_b: int, stats: SimStats) -> list:
        """Quand deux nœuds se rencontrent, échange les messages."""
        forwarded = []
        
        for node_from, node_to in [(node_a, node_b), (node_b, node_a)]:
            buf = self.buffers[node_from]
            to_forward = []
            
            new_buf = deque()
            for msg in buf:
                # Si le message est pour ce nœud ou broadcast
                if msg.destination == node_to or msg.destination is None:
                    if msg.hop_count < msg.ttl:
                        msg.hop_count += 1
                        forwarded.append(msg)
                        stats.total_dtn_hops += 1
                        self.forward_count += 1
                    else:
                        stats.total_messages_expired += 1
                else:
                    new_buf.append(msg)
            
            self.buffers[node_from] = new_buf
        
        return forwarded
    
    def get_buffer_utilization(self, node_id: int) -> float:
        return len(self.buffers[node_id]) / 2000.0

# ==============================================================================
# YGGDRASIL SIMULATOR — IPv6 mesh addressing
# ==============================================================================

class YggdrasilSim:
    """Simulation d'adressage IPv6 mesh style Yggdrasil (arbre cryptographique)."""
    
    def __init__(self):
        self.node_addresses: dict[int, str] = {}  # node_id -> IPv6 hex
        self.tree_depth = 0
    
    def assign_address(self, node_id: int, depth: int = 0) -> str:
        """Génère une adresse IPv6 ULA basée sur la position dans l'arbre."""
        # Format: 200:xxxx:xxxx:xxxx:xxxx:xxxx:xxxx:xxxx
        h = hashlib.sha256(f"yggdrasil:{node_id}:{depth}".encode()).hexdigest()[:32]
        addr = "200"
        for i in range(0, 32, 4):
            addr += f":{h[i:i+4]}"
        self.node_addresses[node_id] = addr
        return addr
    
    def route_distance(self, node_a: int, node_b: int) -> int:
        """Distance dans l'arbre (nombre de sauts logique)."""
        addr_a = self.node_addresses.get(node_a, "")
        addr_b = self.node_addresses.get(node_b, "")
        
        # Trouve le préfixe commun
        common = 0
        for ca, cb in zip(addr_a, addr_b):
            if ca == cb:
                common += 1
            else:
                break
        
        # Plus le préfixe est long, plus les nœuds sont "proches"
        return max(1, 8 - common // 4)
    
    def get_hex_id(self, node_id: int) -> str:
        return self.node_addresses.get(node_id, "unknown")[:16]

# ==============================================================================
# ZK TRANSACTION SIMULATOR — Transactions asynchrones hors-ligne
# ==============================================================================

class ZKTransactionEngine:
    """Moteur de transactions asynchrones ZK (simplifié, type Mina)."""
    
    def __init__(self, env: simpy.Environment):
        self.env = env
        self.pending_txs: deque = deque()
        self.committed_txs: list = []
        self.state_roots: list = []
        self.last_state_root = "genesis"
    
    def create_transaction(self, sender: int, receiver: int, amount: float) -> dict:
        """Crée une transaction signée avec proof simulé."""
        tx = {
            "tx_id": hashlib.md5(f"{sender}:{receiver}:{amount}:{self.env.now}".encode()).hexdigest()[:12],
            "sender": sender,
            "receiver": receiver,
            "amount": amount,
            "timestamp": self.env.now,
            "proof_computed": True,  # simplifié
            "committed": False
        }
        self.pending_txs.append(tx)
        return tx
    
    def process_pending(self, max_batch: int = 100) -> list:
        """Commute les transactions en attente par lots."""
        batch = []
        for _ in range(min(max_batch, len(self.pending_txs))):
            tx = self.pending_txs.popleft()
            tx["committed"] = True
            tx["commit_time"] = self.env.now
            self.committed_txs.append(tx)
            batch.append(tx)
        
        if batch:
            # Nouvel état racine
            h = hashlib.sha256(f"{self.last_state_root}:{len(self.committed_txs)}".encode()).hexdigest()[:16]
            self.last_state_root = h
            self.state_roots.append(h)
        
        return batch
    
    def queue_size(self) -> int:
        return len(self.pending_txs)

# ==============================================================================
# NETWORK SIMULATOR — Simulation physique
# ==============================================================================

class MeshNetwork:
    """Réseau mesh simulé physique."""
    
    def __init__(self, env: simpy.Environment, width_km: float = 10.0, height_km: float = 10.0):
        self.env = env
        self.nodes: dict[int, Node] = {}
        self.width_m = width_km * 1000
        self.height_m = height_km * 1000
        self.dtn_router = DTNRouter(env)
        self.yggdrasil = YggdrasilSim()
        self.zk_engine = ZKTransactionEngine(env)
        self.pow_validator = PoWValidator(difficulty=4)
        self.stats = SimStats()
    
    def add_mobile_node(self, node_id: int) -> Node:
        """Ajoute un nœud mobile."""
        tech_choice = random.choices(
            [TechType.WIFI_AWARE, TechType.BLE, TechType.LORA],
            weights=[0.5, 0.35, 0.15]
        )[0]
        node = Node(
            node_id=node_id,
            is_bridge=False,
            tech=tech_choice,
            x=random.uniform(0, self.width_m),
            y=random.uniform(0, self.height_m),
            velocity=random.uniform(0.2, 1.5),  # 0.2-1.5 m/s (piéton)
            direction=random.uniform(0, 2 * math.pi),
            ai_capable=False
        )
        self.nodes[node_id] = node
        self.yggdrasil.assign_address(node_id)
        return node
    
    def add_bridge_node(self, node_id: int) -> Node:
        """Ajoute un nœud pont desktop/Ethernet."""
        model_size = random.choice([8, 16, 32, 70, 120])  # GB LLM
        node = Node(
            node_id=node_id,
            is_bridge=True,
            tech=TechType.ETHERNET,
            x=random.uniform(0, self.width_m),
            y=random.uniform(0, self.height_m),
            velocity=0.0,
            ai_capable=True,
            ai_model_size_gb=model_size,
            storage_gb=random.choice([256, 512, 1024, 2048]),  # IPFS seeder
            battery_percent=100.0
        )
        self.nodes[node_id] = node
        self.yggdrasil.assign_address(node_id, depth=1)
        return node
    
    def find_neighbors(self, node: Node) -> list[int]:
        """Trouve les voisins dans la portée du nœud."""
        neighbors = []
        profile = TECH_PROFILES[node.tech]
        
        for other_id, other in self.nodes.items():
            if other_id == node.node_id:
                continue
            
            dist = math.sqrt((node.x - other.x)**2 + (node.y - other.y)**2)
            
            # Ethernet connecte tout ce qui est dans 1km (LAN)
            if node.tech == TechType.ETHERNET and dist < 1000:
                neighbors.append(other_id)
            elif node.tech == TechType.LORA and dist <= 5000:
                neighbors.append(other_id)
            elif dist <= profile.range_m:
                neighbors.append(other_id)
        
        return neighbors
    
    def move_nodes(self):
        """Met à jour les positions (marche aléatoire)."""
        for node_id, node in self.nodes.items():
            if node.velocity == 0:
                continue  # desktop ne bouge pas
            
            # Changement aléatoire de direction
            node.direction += random.uniform(-0.3, 0.3)
            
            node.x += math.cos(node.direction) * node.velocity
            node.y += math.sin(node.direction) * node.velocity
            
            # Bounce sur les bords
            if node.x < 0 or node.x > self.width_m:
                node.direction = math.pi - node.direction
                node.x = max(0, min(node.x, self.width_m))
            if node.y < 0 or node.y > self.height_m:
                node.direction = -node.direction
                node.y = max(0, min(node.y, self.height_m))
            
            # Décharge batterie
            node.battery_percent = max(0, node.battery_percent - 0.0001)
    
    def send_message(self, sender_id: int, msg: Message) -> bool:
        """Envoie un message via le réseau."""
        self.stats.total_messages_sent += 1
        
        # PoW check
        if msg.msg_type in ("alert", "mutual_aid"):
            if not self.pow_validator.compute_pow(msg, self.env.now):
                self.stats.pow_fail += 1
                return False
            self.stats.pow_success += 1
        
        if msg.msg_type == "voice":
            self.stats.total_voice_bytes += msg.payload_size_bytes
        elif msg.msg_type == "alert":
            self.stats.total_text_bytes += msg.payload_size_bytes
        
        # Broadcast — stocke chez l'expéditeur et les voisins
        sender = self.nodes.get(sender_id)
        if not sender:
            return False
        
        self.dtn_router.store_message(sender_id, msg)
        neighbors = self.find_neighbors(sender)
        
        # Délivrance directe aux voisins
        for nb_id in neighbors:
            nb = self.nodes[nb_id]
            if msg.destination is None or msg.destination == nb_id:
                self.stats.total_messages_delivered += 1
                self.stats.delivery_latency_samples.append(self.env.now - msg.timestamp)
        
        # Stockage DTN pour forwarding ultérieur
        for nb_id in neighbors[:10]:  # limite pour perf
            self.dtn_router.store_message(nb_id, msg)
        
        return True
    
    def encounter_opportunity(self):
        """Gère les rencontres entre nœuds (échantillonnage pour performance)."""
        # Échantillonne 1000 nœuds pour la simulation
        sample_ids = random.sample(list(self.nodes.keys()), min(1000, len(self.nodes)))
        
        encounters = 0
        for i in range(len(sample_ids)):
            for j in range(i + 1, len(sample_ids)):
                node_a = self.nodes[sample_ids[i]]
                node_b = self.nodes[sample_ids[j]]
                dist = math.sqrt((node_a.x - node_b.x)**2 + (node_a.y - node_b.y)**2)
                
                if dist <= max(TECH_PROFILES[node_a.tech].range_m, TECH_PROFILES[node_b.tech].range_m):
                    self.dtn_router.forward_opportunity(node_a.node_id, node_b.node_id, self.stats)
                    encounters += 1
        
        return encounters
    
    def process_queue_transactions(self):
        """Traite les transactions ZK en attente."""
        batch = self.zk_engine.process_pending(50)
        self.stats.total_tx_processed += len(batch)

# ==============================================================================
# TRAFFIC GENERATOR
# ==============================================================================

class TrafficGenerator:
    """Génère du trafic réaliste sur le réseau."""
    
    def __init__(self, env: simpy.Environment, network: MeshNetwork):
        self.env = env
        self.network = network
    
    def generate_alerts(self, interval: float = 5.0, max_nodes_alert: int = 50):
        """Génère des alertes publiques (280 car max)."""
        mobile_ids = [nid for nid, n in self.network.nodes.items() if not n.is_bridge]
        
        while True:
            yield self.env.timeout(interval)
            
            # PoW difficulté adaptative
            load = min(1.0, self.network.stats.total_messages_sent / 10000)
            self.network.pow_validator.adjust_difficulty(load)
            
            for _ in range(random.randint(5, max_nodes_alert)):
                if not mobile_ids:
                    break
                sender = random.choice(mobile_ids)
                msg = Message(
                    msg_id=hashlib.md5(f"{sender}:{self.env.now}".encode()).hexdigest()[:12],
                    sender_id=sender,
                    msg_type="alert",
                    payload_size_bytes=random.randint(50, 280),
                    ttl=5,
                    timestamp=self.env.now
                )
                self.network.send_message(sender, msg)
    
    def generate_mutual_aid(self, interval: float = 15.0):
        """Génère des demandes d'entraide (hiérarchisées)."""
        mobile_ids = [nid for nid, n in self.network.nodes.items() if not n.is_bridge]
        
        while True:
            yield self.env.timeout(interval)
            
            for _ in range(random.randint(2, 20)):
                if not mobile_ids:
                    break
                sender = random.choice(mobile_ids)
                msg = Message(
                    msg_id=hashlib.md5(f"aid:{sender}:{self.env.now}".encode()).hexdigest()[:12],
                    sender_id=sender,
                    msg_type="mutual_aid",
                    payload_size_bytes=random.randint(100, 500),
                    ttl=8,
                    timestamp=self.env.now,
                    destination=random.choice(mobile_ids) if mobile_ids else None
                )
                self.network.send_message(sender, msg)
    
    def generate_voice_memos(self, interval: float = 60.0):
        """Mémos vocaux Opus 8kbps."""
        mobile_ids = [nid for nid, n in self.network.nodes.items() if not n.is_bridge]
        
        while True:
            yield self.env.timeout(interval)
            
            for _ in range(random.randint(1, 10)):
                if not mobile_ids:
                    break
                sender = random.choice(mobile_ids)
                # 8kbps = 1000 bytes/sec, durée 5-30 sec
                duration = random.uniform(5, 30)
                size = int(duration * 1000)
                msg = Message(
                    msg_id=hashlib.md5(f"voice:{sender}:{self.env.now}".encode()).hexdigest()[:12],
                    sender_id=sender,
                    msg_type="voice",
                    payload_size_bytes=size,
                    ttl=10,
                    timestamp=self.env.now,
                    destination=random.choice(mobile_ids) if mobile_ids else None
                )
                self.network.send_message(sender, msg)
    
    def generate_transactions(self, interval: float = 30.0):
        """Transactions asynchrones ZK hors-ligne."""
        mobile_ids = [nid for nid, n in self.network.nodes.items() if not n.is_bridge]
        
        while True:
            yield self.env.timeout(interval)
            
            for _ in range(random.randint(5, 50)):
                if len(mobile_ids) < 2:
                    break
                sender = random.choice(mobile_ids)
                receiver = random.choice([m for m in mobile_ids if m != sender])
                self.network.zk_engine.create_transaction(
                    sender, receiver,
                    round(random.uniform(1, 100), 2)
                )
            
            self.network.process_queue_transactions()
    
    def generate_ai_queries(self, interval: float = 120.0):
        """Requêtes IA vers les super-oracles desktop."""
        mobile_ids = [nid for nid, n in self.network.nodes.items() if not n.is_bridge]
        oracle_ids = [nid for nid, n in self.network.nodes.items() if n.is_bridge and n.ai_capable]
        
        while True:
            yield self.env.timeout(interval)
            
            for _ in range(random.randint(1, 15)):
                if not mobile_ids or not oracle_ids:
                    break
                sender = random.choice(mobile_ids)
                oracle = random.choice(oracle_ids)
                msg = Message(
                    msg_id=hashlib.md5(f"ai:{sender}:{self.env.now}".encode()).hexdigest()[:12],
                    sender_id=sender,
                    msg_type="ai_query",
                    payload_size_bytes=random.randint(100, 2000),
                    ttl=6,
                    timestamp=self.env.now,
                    destination=oracle
                )
                self.network.send_message(sender, msg)

# ==============================================================================
# SIMULATION RUNNER
# ==============================================================================

def run_simulation(
    sim_duration: float = 3600.0,     # 1 heure simulée
    mobile_count: int = 10000,         # 10k mobile (scaled pour perf)
    bridge_count: int = 1000,          # 1k ponts desktop
    area_km: float = 10.0,
    report_interval: float = 60.0
):
    """Exécute la simulation complète."""
    
    logger.info(f"=== ONDE MESH SIMULATION ===")
    logger.info(f"Mobiles: {mobile_count:,} | Bridges: {bridge_count:,} | Zone: {area_km}km²")
    logger.info(f"Durée: {sim_duration}s | Rapport: toutes les {report_interval}s")
    
    start_time = time.time()
    env = simpy.Environment()
    network = MeshNetwork(env, width_km=area_km, height_km=area_km)
    
    # Création des nœuds
    logger.info("Création des nœuds...")
    for i in range(mobile_count):
        network.add_mobile_node(i)
        if i % 1000 == 0:
            logger.info(f"  Mobiles créés: {i:,}/{mobile_count:,}")
    
    for i in range(bridge_count):
        network.add_bridge_node(mobile_count + i)
    
    logger.info(f"Total nœuds: {len(network.nodes):,}")
    
    # Traffic generators
    traffic = TrafficGenerator(env, network)
    env.process(traffic.generate_alerts(interval=5.0, max_nodes_alert=30))
    env.process(traffic.generate_mutual_aid(interval=15.0))
    env.process(traffic.generate_voice_memos(interval=60.0))
    env.process(traffic.generate_transactions(interval=30.0))
    env.process(traffic.generate_ai_queries(interval=120.0))
    
    # Monitoring périodique
    def monitor():
        while True:
            yield env.timeout(report_interval)
            
            # Mouvement
            network.move_nodes()
            
            # Rencontres (échantillonnage)
            encounters = network.encounter_opportunity()
            
            # Stats
            s = network.stats
            delivery_rate = (s.total_messages_delivered / max(1, s.total_messages_sent)) * 100
            avg_latency = (sum(s.delivery_latency_samples) / max(1, len(s.delivery_latency_samples)))
            tx_queue = network.zk_engine.queue_size()
            
            logger.info(
                f"[t={env.now:7.0f}s] "
                f"Envoyés: {s.total_messages_sent:,} | "
                f"Délivrés: {s.total_messages_delivered:,} ({delivery_rate:.1f}%) | "
                f"DTN hops: {s.total_dtn_hops:,} | "
                f"Encounters: {encounters} | "
                f"Tx queue: {tx_queue} | "
                f"Tx committed: {s.total_tx_processed:,} | "
                f"PoW OK: {s.pow_success} | "
                f"Lat moy: {avg_latency:.1f}s | "
                f"Texte: {s.total_text_bytes/1024:.1f}KB | "
                f"Voix: {s.total_voice_bytes/1024/1024:.1f}MB"
            )
            
            s.queue_depth_history.append({
                "time": env.now,
                "delivered": s.total_messages_delivered,
                "sent": s.total_messages_sent,
                "dtn_hops": s.total_dtn_hops,
                "encounters": encounters,
                "tx_committed": s.total_tx_processed,
                "tx_pending": tx_queue,
                "pow_success": s.pow_success,
                "pow_fail": s.pow_fail
            })
    
    env.process(monitor())
    
    # Exécution
    logger.info("Démarrage de la simulation...")
    env.run(until=sim_duration)
    
    # Rapport final
    elapsed_real = time.time() - start_time
    s = network.stats
    
    report = {
        "simulation_config": {
            "mobile_nodes": mobile_count,
            "bridge_nodes": bridge_count,
            "total_nodes": len(network.nodes),
            "area_km2": area_km * area_km,
            "duration_sec": sim_duration,
            "real_time_sec": round(elapsed_real, 2)
        },
        "network_stats": {
            "total_messages_sent": s.total_messages_sent,
            "total_messages_delivered": s.total_messages_delivered,
            "delivery_rate_percent": round((s.total_messages_delivered / max(1, s.total_messages_sent)) * 100, 2),
            "total_messages_expired": s.total_messages_expired,
            "total_dtn_hops": s.total_dtn_hops,
            "average_latency_seconds": round(sum(s.delivery_latency_samples) / max(1, len(s.delivery_latency_samples)), 3),
            "text_bytes_sent": s.total_text_bytes,
            "voice_bytes_sent": s.total_voice_bytes,
            "pow_success": s.pow_success,
            "pow_fail": s.pow_fail,
            "total_transactions_committed": s.total_tx_processed,
            "zk_queue_remaining": network.zk_engine.queue_size(),
            "total_zk_state_roots": len(network.zk_engine.state_roots),
            "total_encounters": s.queue_depth_history[-1]["encounters"] if s.queue_depth_history else 0
        },
        "node_stats": {
            "mobile": sum(1 for n in network.nodes.values() if not n.is_bridge),
            "bridge": sum(1 for n in network.nodes.values() if n.is_bridge),
            "ai_capable": sum(1 for n in network.nodes.values() if n.ai_capable),
            "avg_battery": round(sum(n.battery_percent for n in network.nodes.values()) / max(1, len(network.nodes)), 2)
        },
        "buffer_history": s.queue_depth_history[-10:]  # 10 derniers points
    }
    
    # Sauvegarde
    os.makedirs("onde/simulation/results", exist_ok=True)
    with open("onde/simulation/results/simulation_report.json", 'w') as f:
        json.dump(report, f, indent=2)
    
    logger.info(f"\n{'='*60}")
    logger.info(f"SIMULATION TERMINÉE en {elapsed_real:.1f}s réels")
    logger.info(f"{'='*60}")
    logger.info(f"Messages envoyés:  {s.total_messages_sent:,}")
    logger.info(f"Messages délivrés: {s.total_messages_delivered:,}")
    logger.info(f"Taux de délivrance: {(s.total_messages_delivered/max(1,s.total_messages_sent))*100:.2f}%")
    logger.info(f"Hops DTN: {s.total_dtn_hops:,}")
    logger.info(f"PoW réussis: {s.pow_success} / échoués: {s.pow_fail}")
    logger.info(f"Transactions ZK: {s.total_tx_processed:,} commitées")
    logger.info(f"Octets texte: {s.total_text_bytes:,} | voix: {s.total_voice_bytes:,}")
    logger.info(f"Rapport sauvegardé: onde/simulation/results/simulation_report.json")
    
    return report

# ==============================================================================
# ENTRY POINT
# ==============================================================================

if __name__ == "__main__":
    # Configuration échelle réduite pour validation rapide
    # Pour la simulation complète, utiliser: mobile_count=500000, bridge_count=50000
    report = run_simulation(
        sim_duration=3600.0,    # 1 heure simulée
        mobile_count=10000,     # 10k mobile
        bridge_count=1000,      # 1k bridges
        area_km=10.0,
        report_interval=120.0
    )
    
    print("\n✅ Simulation terminée avec succès!")
    print(f"   Réseau: {report['simulation_config']['total_nodes']} nœuds simulés")
    print(f"   Délivrance: {report['network_stats']['delivery_rate_percent']}%")
    print(f"   DTN hops: {report['network_stats']['total_dtn_hops']}")
    print(f"   PoW: {report['network_stats']['pow_success']} succès / {report['network_stats']['pow_fail']} échecs")