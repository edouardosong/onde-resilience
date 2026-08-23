//! Phase 3.6 — Observabilité : registre de métriques du nœud ONDE.
//!
//! Compteurs et jauges **thread-safe** fondés uniquement sur
//! `std::sync::atomic` (aucune dépendance nouvelle) et partagés par
//! [`std::sync::Arc`]. L'instrumentation est volontairement bon marché :
//! chaque point de mesure coûte une ou deux opérations atomiques, jamais
//! d'allocation ni de verrou — les points instrumentés (ingestion, outbox
//! gossip, stockage, événements de pairs) sont hors du chemin critique de
//! chiffrement/signature.
//!
//! Sémantique des compteurs de messages (voir [`NodeMetrics`]) :
//! - `messages_ingested` : événements admis par le gate anti-abus ET traités
//!   avec succès (stockés / appliqués) ;
//! - `messages_rejected` : refusés par le gate ou invalides après signature ;
//! - `messages_gossiped` : événements enregistrés comme NOUVEAUX dans
//!   l'outbox gossip (publications locales + première réception d'un événement
//!   signé à relayer) ;
//! - `messages_duplicated` : événements admis dont l'ID était déjà connu
//!   (échos de relais — preuve de connectivité, aucun retraitement).
//!
//! # Exemple
//!
//! ```
//! use std::sync::Arc;
//! use onde_core::metrics::NodeMetrics;
//!
//! let metrics = Arc::new(NodeMetrics::new());
//! metrics.record_ingested();
//! metrics.set_peers(2, 1);
//! let snapshot = metrics.snapshot();
//! assert_eq!(snapshot.metrics.messages_ingested, 1);
//! assert_eq!(snapshot.peers.known, 2);
//! // Sérialisable tel quel pour un endpoint de santé JSON.
//! let json = metrics.snapshot_json();
//! assert!(json.contains("\"status\":\"ok\""));
//! ```
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Instant unix courant en secondes (0 si l'horloge système est invalide).
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Soustraction **saturante** lock-free sur une jauge atomique : ne descend
/// jamais sous 0 (les purges concurrentes avec des ajouts restent cohérentes).
fn saturating_sub(counter: &AtomicU64, value: u64) {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current.saturating_sub(value);
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => current = actual,
        }
    }
}

/// Registre de métriques du nœud — compteurs/jauges atomiques partagés.
///
/// Le même [`Arc<NodeMetrics>`] est détenu par le nœud (producteur : points
/// d'instrumentation) et par le serveur de santé (consommateur : snapshots
/// JSON). Toutes les opérations sont lock-free (`AtomicU64`, ordering
/// `Relaxed` suffisant : ce sont des compteurs indépendants, jamais des
/// garde-fous de synchronisation).
#[derive(Debug)]
pub struct NodeMetrics {
    /// Instant (unix secs) de création du registre — base de l'uptime.
    started_at: AtomicU64,
    /// Événements admis puis traités avec succès.
    messages_ingested: AtomicU64,
    /// Événements refusés (gate anti-abus ou payload signé invalide).
    messages_rejected: AtomicU64,
    /// Événements enregistrés NOUVEAUX dans l'outbox gossip.
    messages_gossiped: AtomicU64,
    /// Événements admis déjà connus (échos de relais dédupliqués).
    messages_duplicated: AtomicU64,
    /// Pairs suivis par le book de présence (jauge).
    peers_known: AtomicU64,
    /// Pairs avec contact récent (< seuil de partition) (jauge).
    peers_synced: AtomicU64,
    /// Messages retenus dans le magasin hiérarchique (jauge).
    storage_events: AtomicU64,
    /// Somme des tailles brutes logiques (pré-compression) (jauge).
    storage_raw_bytes: AtomicU64,
    /// Somme des octets réellement stockés (compressés) (jauge).
    storage_stored_bytes: AtomicU64,
    /// Requêtes servies par l'endpoint de santé (toutes routes).
    health_requests: AtomicU64,
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeMetrics {
    /// Nouveau registre vide, uptime démarrant à l'instant présent.
    pub fn new() -> Self {
        Self {
            started_at: AtomicU64::new(unix_now()),
            messages_ingested: AtomicU64::new(0),
            messages_rejected: AtomicU64::new(0),
            messages_gossiped: AtomicU64::new(0),
            messages_duplicated: AtomicU64::new(0),
            peers_known: AtomicU64::new(0),
            peers_synced: AtomicU64::new(0),
            storage_events: AtomicU64::new(0),
            storage_raw_bytes: AtomicU64::new(0),
            storage_stored_bytes: AtomicU64::new(0),
            health_requests: AtomicU64::new(0),
        }
    }

    // ---- Points d'instrumentation (producteurs) --------------------

    /// Compter un événement admis et traité avec succès.
    pub fn record_ingested(&self) {
        self.messages_ingested.fetch_add(1, Ordering::Relaxed);
    }

    /// Compter un événement refusé (gate ou payload invalide signé).
    pub fn record_rejected(&self) {
        self.messages_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// Compter `n` événements enregistrés nouveaux dans l'outbox gossip
    /// (`n = 0` → no-op).
    pub fn record_gossiped(&self, n: u64) {
        if n > 0 {
            self.messages_gossiped.fetch_add(n, Ordering::Relaxed);
        }
    }

    /// Compter un événement admis déjà connu (écho de relais dédupliqué).
    pub fn record_duplicated(&self) {
        self.messages_duplicated.fetch_add(1, Ordering::Relaxed);
    }

    /// Remplacer les jauges de pairs (appelé après chaque contact pair).
    pub fn set_peers(&self, known: u64, synced: u64) {
        self.peers_known.store(known, Ordering::Relaxed);
        self.peers_synced.store(synced, Ordering::Relaxed);
    }

    /// Compter un message ajouté au magasin hiérarchique.
    pub fn record_storage_added(&self, raw_bytes: u64, stored_bytes: u64) {
        self.storage_events.fetch_add(1, Ordering::Relaxed);
        self.storage_raw_bytes
            .fetch_add(raw_bytes, Ordering::Relaxed);
        self.storage_stored_bytes
            .fetch_add(stored_bytes, Ordering::Relaxed);
    }

    /// Compter `count` messages purgés du magasin (sweep). Les jauges sont
    /// **saturantes** : elles ne peuvent jamais passer sous zéro (pas de
    /// wrapping `u64` dans le JSON de santé).
    pub fn record_storage_removed(&self, count: u64, raw_bytes: u64, stored_bytes: u64) {
        saturating_sub(&self.storage_events, count);
        saturating_sub(&self.storage_raw_bytes, raw_bytes);
        saturating_sub(&self.storage_stored_bytes, stored_bytes);
    }

    /// Compter une requête servie par l'endpoint de santé.
    pub fn record_health_request(&self) {
        self.health_requests.fetch_add(1, Ordering::Relaxed);
    }

    // ---- Lectures (consommateurs : santé, logs) --------------------

    /// Uptime en secondes depuis la création du registre (saturé à 0 si
    /// l'horloge système recule).
    pub fn uptime_secs(&self) -> u64 {
        unix_now().saturating_sub(self.started_at.load(Ordering::Relaxed))
    }

    /// Pairs connus (book de présence).
    pub fn peers_known(&self) -> u64 {
        self.peers_known.load(Ordering::Relaxed)
    }

    /// Pairs synchronisés (contact plus récent que le seuil de partition).
    pub fn peers_synced(&self) -> u64 {
        self.peers_synced.load(Ordering::Relaxed)
    }

    /// Instantané complet sérialisable ([`serde::Serialize`]) — c'est le
    /// corps exact servi par `GET /health`.
    ///
    /// ```
    /// use onde_core::metrics::NodeMetrics;
    /// let m = NodeMetrics::new();
    /// m.record_rejected();
    /// let snap = m.snapshot();
    /// assert_eq!(snap.status, "ok");
    /// assert_eq!(snap.metrics.messages_rejected, 1);
    /// assert_eq!(snap.storage.events, 0);
    /// ```
    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            status: "ok",
            uptime_s: self.uptime_secs(),
            peers: PeerHealth {
                known: self.peers_known(),
                synced: self.peers_synced(),
            },
            metrics: CounterSet {
                messages_ingested: self.messages_ingested.load(Ordering::Relaxed),
                messages_rejected: self.messages_rejected.load(Ordering::Relaxed),
                messages_gossiped: self.messages_gossiped.load(Ordering::Relaxed),
                messages_duplicated: self.messages_duplicated.load(Ordering::Relaxed),
                health_requests: self.health_requests.load(Ordering::Relaxed),
            },
            storage: StorageHealth {
                events: self.storage_events.load(Ordering::Relaxed),
                bytes_raw: self.storage_raw_bytes.load(Ordering::Relaxed),
                bytes_stored: self.storage_stored_bytes.load(Ordering::Relaxed),
            },
        }
    }

    /// Instantané en JSON compact (une ligne) — corps de `/health` et format
    /// du log structuré de démarrage.
    ///
    /// ```
    /// use onde_core::metrics::NodeMetrics;
    /// let json = NodeMetrics::new().snapshot_json();
    /// let v: serde_json::Value = serde_json::from_str(&json).expect("JSON valide");
    /// assert_eq!(v["status"], "ok");
    /// assert!(v["uptime_s"].is_u64());
    /// assert!(v["peers"]["known"].is_u64());
    /// assert!(v["storage"]["events"].is_u64());
    /// ```
    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.snapshot()).unwrap_or_else(|_| "{}".to_string())
    }

    /// Log structuré UNIQUE de démarrage : une ligne JSON contenant le
    /// snapshot complet + identité affichable du nœud. La seed Ed25519 et
    /// toute donnée secrète ne sont JAMAIS incluses.
    ///
    /// Retourne la ligne EXACTEMENT telle qu'elle vient d'être passée à
    /// `tracing::info!` — les tests vérifient la sortie RÉELLE de cette
    /// fonction plutôt qu'une réplique de sa logique (un corps vidé ou
    /// altéré fait échouer le test). En cas de sérialisation impossible,
    /// rien n'est loggé et une ligne dégénérée `"{}"` est retournée.
    pub fn log_startup_snapshot(&self, node_name: &str) -> String {
        let mut value = match serde_json::to_value(self.snapshot()) {
            Ok(serde_json::Value::Object(map)) => map,
            _ => return "{}".to_string(), // sérialisation impossible → pas de log trompeur
        };
        value.insert("event".into(), serde_json::json!("startup"));
        value.insert("node".into(), serde_json::json!(node_name));
        let line = serde_json::Value::Object(value).to_string();
        tracing::info!("{line}");
        line
    }
}

/// Instantané complet du nœud — corps JSON exact de `GET /health`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MetricsSnapshot {
    /// Toujours `"ok"` tant que l'endpoint répond (la santé détaillée vit
    /// dans les jauges ; un nœud qui répond prouve son runtime).
    pub status: &'static str,
    /// Uptime en secondes.
    pub uptime_s: u64,
    /// Vue pairs.
    pub peers: PeerHealth,
    /// Vue compteurs de messages.
    pub metrics: CounterSet,
    /// Vue stockage hiérarchique.
    pub storage: StorageHealth,
}

/// Jauges de connectivité mesh.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PeerHealth {
    /// Pairs suivis (au moins un événement signé reçu).
    pub known: u64,
    /// Pairs vus plus récemment que le seuil de partition (300 s).
    pub synced: u64,
}

/// Compteurs cumulatifs de messages depuis le démarrage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CounterSet {
    /// Admis puis traités (stockés/appliqués).
    pub messages_ingested: u64,
    /// Refusés (gate anti-abus, payload signé invalide).
    pub messages_rejected: u64,
    /// Enregistrés nouveaux dans l'outbox gossip (à propager).
    pub messages_gossiped: u64,
    /// Admis déjà connus (dédupliqués sans retraitement).
    pub messages_duplicated: u64,
    /// Requêtes servies par l'endpoint de santé.
    pub health_requests: u64,
}

/// Jauges de stockage hiérarchique.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StorageHealth {
    /// Messages retenus (tous tiers).
    pub events: u64,
    /// Tailles brutes cumulées (pré-compression), octets.
    pub bytes_raw: u64,
    /// Octets réellement stockés (payload compressés).
    pub bytes_stored: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn test_counters_start_at_zero() {
        let m = NodeMetrics::new();
        let snap = m.snapshot();
        assert_eq!(snap.status, "ok");
        assert_eq!(snap.metrics.messages_ingested, 0);
        assert_eq!(snap.metrics.messages_rejected, 0);
        assert_eq!(snap.metrics.messages_gossiped, 0);
        assert_eq!(snap.metrics.messages_duplicated, 0);
        assert_eq!(snap.peers.known, 0);
        assert_eq!(snap.storage.events, 0);
    }

    #[test]
    fn test_counter_increments_and_gauges() {
        let m = NodeMetrics::new();
        m.record_ingested();
        m.record_ingested();
        m.record_rejected();
        m.record_gossiped(3);
        m.record_duplicated();
        m.set_peers(7, 5);
        m.record_storage_added(1000, 400);
        m.record_storage_removed(1, 600, 250);
        let snap = m.snapshot();
        assert_eq!(snap.metrics.messages_ingested, 2);
        assert_eq!(snap.metrics.messages_rejected, 1);
        assert_eq!(snap.metrics.messages_gossiped, 3);
        assert_eq!(snap.metrics.messages_duplicated, 1);
        assert_eq!(snap.peers.known, 7);
        assert_eq!(snap.peers.synced, 5);
        assert_eq!(snap.storage.events, 0); // 1 ajout − 1 purge
        assert_eq!(snap.storage.bytes_raw, 400);
        assert_eq!(snap.storage.bytes_stored, 150);
        m.record_health_request();
        assert_eq!(m.snapshot().metrics.health_requests, 1);
    }

    #[test]
    fn test_snapshot_json_is_parseable_and_complete() {
        let m = NodeMetrics::new();
        m.set_peers(3, 2);
        let v: serde_json::Value = serde_json::from_str(&m.snapshot_json()).unwrap();
        assert_eq!(v["status"], "ok");
        assert!(v["uptime_s"].is_u64());
        assert_eq!(v["peers"]["known"], 3);
        assert_eq!(v["peers"]["synced"], 2);
        for field in [
            "messages_ingested",
            "messages_rejected",
            "messages_gossiped",
            "messages_duplicated",
            "health_requests",
        ] {
            assert!(v["metrics"][field].is_u64(), "missing metrics.{field}");
        }
        assert!(v["storage"]["events"].is_u64());
        assert!(v["storage"]["bytes_raw"].is_u64());
        assert!(v["storage"]["bytes_stored"].is_u64());
    }

    #[test]
    fn test_concurrent_counter_updates_are_exact() {
        let shared = Arc::new(NodeMetrics::new());
        let threads = 8usize;
        let per_thread = 10_000u64;
        let mut handles = Vec::new();
        for t in 0..threads {
            let m = shared.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..per_thread {
                    m.record_ingested();
                    if i % 2 == 0 {
                        m.record_duplicated();
                    }
                    if i == 0 && t == 0 {
                        m.record_gossiped(5);
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("worker thread must not panic");
        }
        let snap = shared.snapshot();
        assert_eq!(
            snap.metrics.messages_ingested,
            (threads * per_thread as usize) as u64
        );
        assert_eq!(
            snap.metrics.messages_duplicated,
            threads as u64 * per_thread / 2
        );
        assert_eq!(snap.metrics.messages_gossiped, 5);
    }

    #[test]
    fn test_storage_accounting_never_negative() {
        // fetch_sub sur une jauge à zéro sature à 0 (wrapping atomique
        // maîtrisé côté appelant : purge seulement après ajout mesuré).
        let m = NodeMetrics::new();
        m.record_storage_removed(1, 10, 10);
        assert_eq!(m.snapshot().storage.events, 0);
        m.record_storage_added(10, 4);
        m.record_storage_added(30, 12);
        m.record_storage_removed(1, 30, 12);
        let s = m.snapshot().storage;
        assert_eq!((s.events, s.bytes_raw, s.bytes_stored), (1, 10, 4));
    }

    #[test]
    fn test_startup_snapshot_log_is_single_line_json() {
        // On appelle la fonction RÉELLE et on vérifie SA sortie (et non une
        // réplique de sa logique) : un corps vidé ou altéré par un mutant
        // fait échouer ce test.
        let m = NodeMetrics::new();
        m.record_ingested();
        m.set_peers(3, 2);
        let line = m.log_startup_snapshot("test-node");
        assert!(!line.contains('\n'), "single line required, got: {line}");
        let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON line");
        assert_eq!(v["event"], "startup");
        assert_eq!(v["node"], "test-node");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["peers"]["known"], 3);
        assert_eq!(v["metrics"]["messages_ingested"], 1);
        // Assertion numérique saine sur l'uptime (pas seulement is_u64).
        let uptime = v["uptime_s"].as_u64().expect("uptime_s numeric");
        assert!(
            uptime <= 60,
            "fresh registry uptime must be small, got {uptime}"
        );
    }

    #[test]
    fn test_uptime_tracks_measured_wall_time() {
        // L'uptime doit avancer avec le temps mesuré : tue les mutants
        // unix_now→{0,1} et uptime_secs→{0,1}.
        let start = std::time::Instant::now();
        let m = NodeMetrics::new();
        std::thread::sleep(Duration::from_millis(2100));
        let wall = start.elapsed().as_secs(); // ≥ 2 s mesurées
        assert!(
            wall >= 2,
            "test premise: at least 2s must elapse, got {wall}"
        );
        let u = m.uptime_secs();
        assert!(
            u >= wall,
            "uptime must track measured time: u={u} wall={wall}"
        );
        assert!(
            u <= wall + 1,
            "uptime must stay bounded by measured wall clock: u={u} wall={wall}"
        );
        // Le JSON de snapshot expose bien cette même valeur numérique.
        let v: serde_json::Value = serde_json::from_str(&m.snapshot_json()).unwrap();
        assert_eq!(v["uptime_s"].as_u64(), Some(u));
    }
}
