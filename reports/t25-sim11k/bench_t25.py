#!/usr/bin/env python3
"""T25 — Driver de montée en charge simulation ONDE (data-sim).

Mesure wall-time / RAM pic / métriques réseau pour un palier donné.
N'instrumente PAS le modèle : capture des échantillons de latence par
monkeypatch ADDITIF de SimStats.register_unique_delivery (l'appel original
est toujours exécuté ; seule une copie externe des latences acceptées est
gardée pour calculer la p95 — même jeu d'échantillons que la moyenne).

Usage : bench_t25.py MOBILE BRIDGE [DURATION] [SEED] [LABEL]
Sortie : ligne RESULT_JSON={...} sur stdout ; logs SimPy complets sur stdout.
"""
import sys, time, json, resource, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2]   # racine worktree
sys.path.insert(0, str(ROOT / "simulation"))

mobile = int(sys.argv[1]); bridge = int(sys.argv[2])
duration = float(sys.argv[3]) if len(sys.argv) > 3 else 3600.0
seed = int(sys.argv[4]) if len(sys.argv) > 4 else 42
label = sys.argv[5] if len(sys.argv) > 5 else f"{mobile+bridge}"

import mesh_sim

lat_captured = []
_orig_register = mesh_sim.SimStats.register_unique_delivery

def _register_patched(self, sender_id, msg_id, latency):
    ok = _orig_register(self, sender_id, msg_id, latency)
    if ok:
        lat_captured.append(latency)
    return ok

mesh_sim.SimStats.register_unique_delivery = _register_patched

t0 = time.perf_counter()
report = mesh_sim.run_simulation(
    sim_duration=duration, mobile_count=mobile, bridge_count=bridge,
    area_km=10.0, report_interval=60.0, seed=seed)
wall = time.perf_counter() - t0

peak_mb = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss / 1024.0
lat_sorted = sorted(lat_captured)
n = len(lat_sorted)
def pct(p):
    return lat_sorted[min(n - 1, int(round(p / 100.0 * (n - 1))))] if n else 0.0

ns = report["network_stats"]
cfg = report["simulation_config"]
result = {
    "label": label,
    "nodes_total": cfg["total_nodes"],
    "mobile": mobile, "bridge": bridge,
    "duration_sim_s": duration, "seed": seed,
    "wall_time_s": round(wall, 2),
    "real_time_report_s": cfg["real_time_sec"],
    "ram_peak_mb": round(peak_mb, 1),
    "messages_sent": ns["total_messages_sent"],
    "delivered_unique": ns["delivered_unique_messages"],
    "expired_unique": ns["expired_unique_messages"],
    "delivery_rate_percent": ns["delivery_rate_percent"],
    "total_dtn_hops": ns["total_dtn_hops"],
    "avg_latency_s": ns["average_latency_seconds"],
    "p95_latency_s": round(pct(95), 3),
    "p50_latency_s": round(pct(50), 3),
    "latency_samples": n,
    "throughput_msg_per_sim_s": round(ns["total_messages_sent"] / duration, 3),
    "processing_msgs_per_wall_s": round(ns["total_messages_sent"] / wall, 1),
    "pow_success": ns["pow_success"], "pow_fail": ns["pow_fail"],
    "tx_committed": ns["total_transactions_committed"],
    "encounters_last_tick": ns["total_encounters"],
}
print("RESULT_JSON=" + json.dumps(result))

# copie du rapport JSON du palier comme artefact brut
raw_dir = pathlib.Path(__file__).resolve().parent / "raw"
raw_dir.mkdir(exist_ok=True)
(raw_dir / f"{label}_report.json").write_text(json.dumps(report, indent=2))
