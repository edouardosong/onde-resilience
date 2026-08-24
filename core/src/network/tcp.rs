//! Transport TCP/IP réel pour le mesh ONDE (Phase A — appareils réels, T32).
//!
//! Ce module branche le flux gossip existant sur de vraies sockets TCP
//! (`std::net` + threads std uniquement — zéro dépendance ajoutée), pour le
//! scénario « deux appareils Android sur le même réseau local sans internet ».
//!
//! # Framing (format câblé)
//!
//! ```text
//! [u32 BE longueur][payload = octets wire padés MeshEvent::to_wire_bytes]
//! ```
//!
//! Le payload est exactement la sortie de [`crate::protocol::MeshEvent::
//! to_wire_bytes`] (padé au seau `TrafficPadding`). Toute frame déclarant un
//! payload > [`MAX_FRAME_PAYLOAD`] (= seau de padding maximal 16 384 o) ou de
//! longueur nulle est une **violation de protocole** : la connexion est fermée
//! poliment (log structuré, pas de panique, jamais de lecture du corps).
//! Le receveur décode ensuite via `MeshEvent::from_wire_bytes` — même chemin
//! qu'un pair gossip.
//!
//! # Rôles
//!
//! - **Serveur** (`--listen`) : boucle d'acceptation threadée BORNÉE, calquée
//!   sur le serveur de santé testé en T23 ([`crate::health`]) : listener non
//!   bloquant, plafond atomique de connexions actives appliqué DANS le parent
//!   avant tout spawn, refus inline au-delà du plafond, cap d'erreurs
//!   consécutives, arrêt propre par fanion. Chaque connexion acceptée ne fait
//!   que LIRE (l'émission passe toujours par nos connexions clientes).
//! - **Client** (un fil par pair configuré) : connexion avec timeout borné,
//!   reconnexion périodique bornée (`reconnect_interval`), moitié écriture sur
//!   fil dédié (queue sortante par pair, bornée), moitié lecture sur le fil
//!   client. Les frames lues sur une connexion cliente sont traitées comme
//!   celles d'une connexion servante (flux bidirectionnel).
//!
//! # Intégration Node — [`process_inbound`] / [`flush_outbound`]
//!
//! Le `Node` n'est PAS partagé entre fils : un **pump** tourne sur le fil
//! propriétaire des nœuds (binaire `onde_node`, tests e2e) :
//!
//! 1. [`process_inbound`] vide la queue entrante du transport et rejoue pour
//!    chaque frame EXACTEMENT le chemin d'un événement reçu d'un pair gossip :
//!    `from_wire_bytes` → gate d'admission + routage métier
//!    ([`crate::node::Node::receive_peer_event`]) → métriques/réputation.
//! 2. [`flush_outbound`] tire les événements pending du gossip pour chaque
//!    pair configuré ([`GossipProtocol::peek_pending_for_peer`]), les encode,
//!    les met en file sortante, et ne marque « livré » qu'après mise en file
//!    réussie (même sémantique que [`crate::node::Node::take_heal_batch`]).
//!
//! Perte tolérée : le modèle gossip est épidémique — une frame perdue (queue
//! pleine, coupure) sera re-proposée par tout relais qui ne l'a pas encore
//! marquée livrée pour ce pair ; les doublons sont dédupliqués par ID côté
//! receveur sans pénalité de réputation.
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::node::Node;
use crate::protocol::MeshEvent;

// ────────────────────────────────────────────────────────────────────────
// Framing
// ────────────────────────────────────────────────────────────────────────

/// Longueur de l'en-tête de frame : `u32` big-endian.
pub const FRAME_HEADER_LEN: usize = 4;

/// Payload maximal d'une frame : le seau de padding le plus grand
/// ([`crate::crypto::TrafficPadding::BUCKETS`] dernier élément). Un wire
/// padé ne dépasse jamais cette taille ; au-delà = violation de protocole.
pub const MAX_FRAME_PAYLOAD: usize = 16_384;

/// Encoder une frame : `u32 BE longueur` + payload. Erreur propre (jamais de
/// panique) si le payload dépasse [`MAX_FRAME_PAYLOAD`] — l'appelant décide
/// (log + abandon de la frame ; le gossip re-proposera l'événement plus tard).
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_FRAME_PAYLOAD {
        return Err(format!(
            "tcp frame payload too large: {} bytes (max {MAX_FRAME_PAYLOAD})",
            payload.len()
        ));
    }
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// État d'un pas de lecture incrémental ([`FrameReader::poll`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramePoll {
    /// Rien de complet disponible pour l'instant (socket à timeout court).
    Pending,
    /// Une frame complète est disponible : payload seul (en-tête retiré).
    Frame(Vec<u8>),
    /// Fin de flux PROPRE : le pair a fermé entre deux frames.
    Eof,
    /// Violation de protocole ou coupure en plein milieu d'une frame :
    /// la connexion doit être fermée (refus poliment, log structuré).
    Fatal(String),
}

/// Lecteur de frames incrémental, tolérant aux timeouts de socket.
///
/// Contrairement à un `read_exact` bloquant, `poll` conserve son état entre
/// deux appels : un socket en mode timeout (`WouldBlock`/`TimedOut`) rend
/// [`FramePoll::Pending`] sans perdre les octets déjà lus, ce qui laisse le
/// fil lecteur sonder le fanion d'arrêt régulièrement (arrêt propre ≤ budget
/// socket). Les erreurs réelles et les troncatures produisent
/// [`FramePoll::Fatal`].
///
/// Garde-fous appliqués dès l'en-tête (avant TOUTE lecture du corps) :
/// - longueur déclarée > [`MAX_FRAME_PAYLOAD`] → `Fatal` (refus poliment) ;
/// - longueur déclarée 0 → `Fatal` (un événement wire n'est jamais vide).
#[derive(Debug, Default)]
pub struct FrameReader {
    header: [u8; FRAME_HEADER_LEN],
    header_fill: usize,
    body: Vec<u8>,
    body_fill: usize,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Vrai tant qu'aucun octet d'une frame en cours n'a été consommé
    /// (utilisé pour distinguer une fermeture propre d'une troncature).
    fn between_frames(&self) -> bool {
        self.header_fill == 0 && self.body.is_empty()
    }

    fn reset(&mut self) {
        self.header_fill = 0;
        self.body.clear();
        self.body_fill = 0;
    }

    /// Tenter un pas de lecture sur `r`. Coût O(octets disponibles) ;
    /// ne bloque jamais plus qu'un appel `read` sous-jacent.
    pub fn poll<R: Read>(&mut self, r: &mut R) -> FramePoll {
        loop {
            if self.header_fill < FRAME_HEADER_LEN {
                match r.read(&mut self.header[self.header_fill..]) {
                    Ok(0) => {
                        return if self.between_frames() {
                            FramePoll::Eof
                        } else {
                            FramePoll::Fatal(
                                "tcp frame truncated in header (peer closed mid-frame)".to_string(),
                            )
                        };
                    }
                    Ok(n) => self.header_fill += n,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        return FramePoll::Pending;
                    }
                    Err(e) => return FramePoll::Fatal(format!("tcp read header failed: {e}")),
                }
                if self.header_fill < FRAME_HEADER_LEN {
                    continue; // en-tête partiel → nouvel appel read
                }
                // En-tête complet : valider AVANT toute lecture du corps.
                let declared_raw = u32::from_be_bytes(self.header);
                if declared_raw == 0 {
                    self.reset();
                    return FramePoll::Fatal("tcp frame declares zero length".to_string());
                }
                if declared_raw as usize > MAX_FRAME_PAYLOAD {
                    self.reset();
                    return FramePoll::Fatal(format!(
                        "tcp frame too large: declared {declared_raw} bytes (max {MAX_FRAME_PAYLOAD})"
                    ));
                }
                let declared = declared_raw as usize;
                self.body = vec![0u8; declared];
            } else {
                match r.read(&mut self.body[self.body_fill..]) {
                    Ok(0) => {
                        return FramePoll::Fatal(
                            "tcp frame truncated in payload (peer closed mid-frame)".to_string(),
                        );
                    }
                    Ok(n) => self.body_fill += n,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) =>
                    {
                        return FramePoll::Pending;
                    }
                    Err(e) => return FramePoll::Fatal(format!("tcp read payload failed: {e}")),
                }
                if self.body_fill == self.body.len() {
                    let payload = std::mem::take(&mut self.body);
                    self.reset();
                    return FramePoll::Frame(payload);
                }
            }
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Transport threadé borné (patterns T23 health.rs)
// ────────────────────────────────────────────────────────────────────────

/// Intervalle de sonde de la boucle d'acceptation non bloquante.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// Intervalle de sonde des fils d'écriture quand leur queue est vide.
const WRITER_POLL_INTERVAL: Duration = Duration::from_millis(20);
/// Cap d'erreurs d'acceptation consécutives avant abandon du listener
/// (même politique que [`crate::health`]).
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 32;
/// Plafond par défaut de connexions simultanées servies.
const DEFAULT_MAX_CONNECTIONS: usize = 32;
/// Capacité par défaut des queues entrante (globale) et sortante (par pair).
const DEFAULT_QUEUE_CAPACITY: usize = 256;
/// Reconnexion par défaut vers un pair configuré.
const DEFAULT_RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
/// Timeout de connexion sortante par défaut.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Budget par opération socket par défaut (lecture/écriture). Borne aussi la
/// latence maximale d'arrêt propre : chaque fil sonde le fanion entre deux
/// opérations au plus tard après ce délai.
const DEFAULT_SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration d'un [`TcpTransport`] (voir [`Default`] pour les bornes).
#[derive(Debug, Clone)]
pub struct TcpTransportConfig {
    /// Adresse d'écoute serveur ; `None` = client uniquement.
    pub listen: Option<SocketAddr>,
    /// Pairs à joindre en client (reconnexion périodique bornée).
    pub peers: Vec<SocketAddr>,
    /// Plafond de connexions servies simultanément.
    pub max_connections: usize,
    /// Délai entre deux tentatives de reconnexion vers un pair.
    pub reconnect_interval: Duration,
    /// Timeout d'une connexion sortante.
    pub connect_timeout: Duration,
    /// Budget par opération socket (lecture/écriture).
    pub socket_timeout: Duration,
    /// Capacité max : queue entrante globale et queues sortantes par pair.
    pub queue_capacity: usize,
}

impl Default for TcpTransportConfig {
    fn default() -> Self {
        Self {
            listen: None,
            peers: Vec::new(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            reconnect_interval: DEFAULT_RECONNECT_INTERVAL,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            socket_timeout: DEFAULT_SOCKET_TIMEOUT,
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
        }
    }
}

/// Une frame reçue, associée à sa clé de pair source (adresse texte).
#[derive(Debug, Clone)]
pub struct InboundFrame {
    pub peer_key: String,
    pub payload: Vec<u8>,
}

/// Compteurs cumulés du transport (snapshot atomique, sans verrou).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransportStats {
    pub frames_sent: u64,
    pub frames_received: u64,
    pub frames_dropped_inbound_full: u64,
    pub frames_dropped_outbound_full: u64,
    pub connections_refused_busy: u64,
    pub reconnect_attempts: u64,
    pub protocol_violations: u64,
}

struct Shared {
    stop: AtomicBool,
    config: TcpTransportConfig,
    /// Adresse EFFECTIVE du serveur après bind (résout le bind éphémère `:0`).
    listener_addr: Mutex<Option<SocketAddr>>,
    /// Queue entrante globale (drainée par le pump sur le fil propriétaire).
    inbound: Mutex<VecDeque<InboundFrame>>,
    /// Queue sortante PAR PAIR (clé = adresse texte du pair).
    outbound: Mutex<HashMap<String, VecDeque<Vec<u8>>>>,
    /// Cibles de numérotation déclarées (garde anti-doublon de fils clients).
    dial_targets: Mutex<HashSet<String>>,
    active_conns: AtomicUsize,
    stats: TransportStatsAtomic,
}

/// Compteurs atomiques internes (miroir écrivable de [`TransportStats`]).
#[derive(Debug, Default)]
struct TransportStatsAtomic {
    frames_sent: AtomicU64,
    frames_received: AtomicU64,
    frames_dropped_inbound_full: AtomicU64,
    frames_dropped_outbound_full: AtomicU64,
    connections_refused_busy: AtomicU64,
    reconnect_attempts: AtomicU64,
    protocol_violations: AtomicU64,
}

impl TransportStatsAtomic {
    fn snapshot(&self) -> TransportStats {
        TransportStats {
            frames_sent: self.frames_sent.load(Ordering::Relaxed),
            frames_received: self.frames_received.load(Ordering::Relaxed),
            frames_dropped_inbound_full: self.frames_dropped_inbound_full.load(Ordering::Relaxed),
            frames_dropped_outbound_full: self.frames_dropped_outbound_full.load(Ordering::Relaxed),
            connections_refused_busy: self.connections_refused_busy.load(Ordering::Relaxed),
            reconnect_attempts: self.reconnect_attempts.load(Ordering::Relaxed),
            protocol_violations: self.protocol_violations.load(Ordering::Relaxed),
        }
    }
}

/// Verrou tolérant au poison : un fil qui paniquerait ne doit pas bloquer
/// définitivement le transport. Les sections critiques ici sont triviales
/// (push/pop bornés) et ne laissent jamais d'invariant cassé derrière elles.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Transport TCP réel : serveur d'écoute borné + fils clients par pair.
///
/// Cycle de vie : [`TcpTransport::start`] lie le listener (si configuré) et
/// démarre un fil client par pair ; [`TcpTransport::add_peer`] ajoute une
/// cible à chaud ; [`TcpTransport::stop`] arme le fanion d'arrêt — tous les
/// fils sortent au plus tard après `socket_timeout` (aucun join bloquant :
/// les fils sont détachés et meurent seuls, le port est libéré par la
/// fermeture du listener possédé par son unique fil d'acceptation).
pub struct TcpTransport {
    shared: Arc<Shared>,
}

impl TcpTransport {
    /// Construire le transport (pas encore démarré).
    pub fn new(config: TcpTransportConfig) -> Self {
        Self {
            shared: Arc::new(Shared {
                stop: AtomicBool::new(false),
                listener_addr: Mutex::new(None),
                config,
                inbound: Mutex::new(VecDeque::new()),
                outbound: Mutex::new(HashMap::new()),
                dial_targets: Mutex::new(HashSet::new()),
                active_conns: AtomicUsize::new(0),
                stats: TransportStatsAtomic::default(),
            }),
        }
    }

    /// Démarrer serveur + fils clients configurés. Erreur propre si le bind
    /// échoue (port occupé…) — l'appelant décide (fatal côté daemon).
    pub fn start(&self) -> Result<(), String> {
        if let Some(addr) = self.shared.config.listen {
            let listener = TcpListener::bind(addr)
                .map_err(|e| format!("tcp transport bind {addr} failed: {e}"))?;
            listener
                .set_nonblocking(true)
                .map_err(|e| format!("tcp transport nonblocking setup failed: {e}"))?;
            let effective = listener
                .local_addr()
                .map_err(|e| format!("tcp transport local_addr failed: {e}"))?;
            let shared = self.shared.clone();
            std::thread::Builder::new()
                .name("onde-tcp-accept".to_string())
                .spawn(move || accept_loop(listener, shared))
                .map_err(|e| format!("tcp accept thread spawn failed: {e}"))?;
            *lock_or_recover(&self.shared.listener_addr) = Some(effective);
            tracing::info!(target: "onde_core::network", "event=tcp_listen addr={effective} max_conns={}", self.shared.config.max_connections);
        }
        for addr in &self.shared.config.peers {
            self.add_peer(*addr);
        }
        Ok(())
    }

    /// Adresse EFFECTIVE du serveur après bind — résout le bind éphémère
    /// `:0` (port choisi par l'OS). `None` si client uniquement ou pas
    /// encore démarré.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        *lock_or_recover(&self.shared.listener_addr)
    }

    /// Ajouter une cible client à chaud (idempotent par adresse). Le fil
    /// client reconnexion périodique bornée démarre immédiatement.
    pub fn add_peer(&self, addr: SocketAddr) {
        let key = addr.to_string();
        {
            let mut targets = lock_or_recover(&self.shared.dial_targets);
            if !targets.insert(key.clone()) {
                return; // déjà en dial — pas de second fil pour ce pair
            }
        }
        // Réserver la queue sortante du pair dès maintenant.
        lock_or_recover(&self.shared.outbound)
            .entry(key.clone())
            .or_default();
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("onde-tcp-dial-{key}"))
            .spawn(move || client_loop(key, addr, shared));
        if let Err(e) = spawned {
            tracing::warn!(target: "onde_core::network", "event=tcp_dial_thread_failed addr={addr} error={e}");
            lock_or_recover(&self.shared.dial_targets).remove(&addr.to_string());
        }
    }

    /// Clés de pairs connus (cibles de numérotation déclarées) — le pump
    /// tire les événements gossip pending pour chacune d'elles.
    pub fn peer_keys(&self) -> Vec<String> {
        let targets = lock_or_recover(&self.shared.dial_targets);
        let mut keys: Vec<String> = targets.iter().cloned().collect();
        keys.sort();
        keys
    }

    /// Mettre une frame wire en file sortante vers `peer_key`.
    ///
    /// Erreur propre si le payload dépasse [`MAX_FRAME_PAYLOAD`]. Queue
    /// pleine → frame la plus ancienne évincée (compteur dédié) : perte
    /// tolérée par le modèle gossip épidémique, jamais de panique ni de
    /// blocage de l'appelant.
    pub fn enqueue_outbound(&self, peer_key: &str, payload: Vec<u8>) -> Result<(), String> {
        if payload.len() > MAX_FRAME_PAYLOAD {
            return Err(format!(
                "tcp outbound payload too large: {} bytes (max {MAX_FRAME_PAYLOAD})",
                payload.len()
            ));
        }
        let mut map = lock_or_recover(&self.shared.outbound);
        let queue = map.entry(peer_key.to_string()).or_default();
        let capacity = self.shared.config.queue_capacity;
        while queue.len() >= capacity {
            queue.pop_front();
            self.shared
                .stats
                .frames_dropped_outbound_full
                .fetch_add(1, Ordering::Relaxed);
        }
        queue.push_back(payload);
        Ok(())
    }

    /// Vider la queue entrante (appelé par le pump sur le fil propriétaire
    /// des nœuds). Retourne les frames dans l'ordre d'arrivée.
    pub fn drain_inbound(&self) -> Vec<InboundFrame> {
        let mut queue = lock_or_recover(&self.shared.inbound);
        queue.drain(..).collect()
    }

    /// Snapshot des compteurs (test + observabilité).
    pub fn stats(&self) -> TransportStats {
        self.shared.stats.snapshot()
    }

    /// Nombre de connexions actives servies/plafonnées (visibilité).
    pub fn active_connections(&self) -> usize {
        self.shared.active_conns.load(Ordering::Relaxed)
    }

    /// Arrêt propre : arme le fanion ; tous les fils sondent entre deux
    /// opérations socket au plus tard après `socket_timeout`.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

// ── Serveur ─────────────────────────────────────────────────────────────

/// Boucle d'acceptation bornée — calquée sur [`crate::health`] (T23) :
/// listener non bloquant possédé par ce seul fil, plafond atomique de
/// connexions appliqué DANS le parent AVANT tout spawn (pas de fenêtre
/// TOCTOU), refus inline au-delà du plafond, cap d'erreurs consécutives.
fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    let mut consecutive_errors: u32 = 0;
    while !shared.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                consecutive_errors = 0;
                // Admission atomique dans le parent, avant tout spawn.
                if shared.active_conns.fetch_add(1, Ordering::AcqRel)
                    >= shared.config.max_connections
                {
                    shared.active_conns.fetch_sub(1, Ordering::Release);
                    shared
                        .stats
                        .connections_refused_busy
                        .fetch_add(1, Ordering::Relaxed);
                    // Refus poliment : fermeture propre (shutdown BOTH avant
                    // drop pour éviter un RST sur requête déjà reçue).
                    let _ = stream.shutdown(Shutdown::Both);
                    tracing::warn!(target: "onde_core::network", "event=tcp_conn_refused peer={peer_addr} reason=busy");
                    continue;
                }
                let s = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name("onde-tcp-serve".to_string())
                    .spawn(move || {
                        read_frames_until_closed(stream, peer_addr.to_string(), &s);
                        s.active_conns.fetch_sub(1, Ordering::Release);
                    });
                if spawned.is_err() {
                    // Épuisement de threads OS : on ignore cette connexion
                    // (le pair retentera), quota rendu immédiatement.
                    shared.active_conns.fetch_sub(1, Ordering::Release);
                    tracing::warn!(target: "onde_core::network", "event=tcp_serve_thread_spawn_failed peer={peer_addr}");
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) => {
                if shared.stop.load(Ordering::Acquire) {
                    break;
                }
                consecutive_errors += 1;
                tracing::warn!(target: "onde_core::network", "event=tcp_accept_error count={consecutive_errors} error={e}");
                if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    tracing::error!(target: "onde_core::network", "event=tcp_listener_died consecutive_errors={consecutive_errors}");
                    return;
                }
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
        }
    }
    tracing::info!(target: "onde_core::network", "event=tcp_accept_stopped reason=shutdown");
}

/// Configurer une socket de session (nodelay + budgets par opération).
fn configure_stream(stream: &TcpStream, socket_timeout: Duration) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(socket_timeout));
    let _ = stream.set_write_timeout(Some(socket_timeout));
}

/// Lire les frames d'une connexion jusqu'à fermeture/erreur/arrêt, en
/// poussant chaque frame complète dans la queue entrante. Utilisé tel quel
/// par les connexions SERVANTES et CLIENTES (flux bidirectionnel).
fn read_frames_until_closed(mut stream: TcpStream, peer_key: String, shared: &Arc<Shared>) {
    configure_stream(&stream, shared.config.socket_timeout);
    let mut reader = FrameReader::new();
    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        match reader.poll(&mut stream) {
            FramePoll::Pending => continue,
            FramePoll::Frame(payload) => push_inbound(shared, peer_key.clone(), payload),
            FramePoll::Eof => {
                tracing::debug!(target: "onde_core::network", "event=tcp_peer_disconnected peer={peer_key}");
                break;
            }
            FramePoll::Fatal(reason) => {
                shared
                    .stats
                    .protocol_violations
                    .fetch_add(1, Ordering::Relaxed);
                // Refus poliment : log structuré puis fermeture. Le pair
                // légitime se reconnectera ; l'attaquant ne gagne rien.
                tracing::warn!(target: "onde_core::network", "event=tcp_protocol_violation peer={peer_key} action=close_connection reason={reason}");
                break;
            }
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Push borné dans la queue entrante ; queue pleine → frame la plus ancienne
/// évincée (compteur dédié). Jamais de blocage du fil lecteur.
fn push_inbound(shared: &Arc<Shared>, peer_key: String, payload: Vec<u8>) {
    let mut queue = lock_or_recover(&shared.inbound);
    let capacity = shared.config.queue_capacity;
    while queue.len() >= capacity {
        queue.pop_front();
        shared
            .stats
            .frames_dropped_inbound_full
            .fetch_add(1, Ordering::Relaxed);
    }
    queue.push_back(InboundFrame { peer_key, payload });
    shared.stats.frames_received.fetch_add(1, Ordering::Relaxed);
}

// ── Client ──────────────────────────────────────────────────────────────

/// Fil client d'un pair : connexions répétées avec attente BORNÉE entre
/// tentatives, jusqu'à arrêt du transport.
fn client_loop(peer_key: String, addr: SocketAddr, shared: Arc<Shared>) {
    tracing::info!(target: "onde_core::network", "event=tcp_dial_start peer={peer_key}");
    while !shared.stop.load(Ordering::Acquire) {
        match TcpStream::connect_timeout(&addr, shared.config.connect_timeout) {
            Ok(stream) => {
                tracing::info!(target: "onde_core::network", "event=tcp_connected peer={peer_key}");
                run_client_connection(stream, peer_key.clone(), &shared);
                if !shared.stop.load(Ordering::Acquire) {
                    sleep_bounded(shared.config.reconnect_interval, &shared.stop);
                }
            }
            Err(e) => {
                shared
                    .stats
                    .reconnect_attempts
                    .fetch_add(1, Ordering::Relaxed);
                tracing::debug!(target: "onde_core::network", "event=tcp_connect_failed peer={peer_key} error={e}");
                sleep_bounded(shared.config.reconnect_interval, &shared.stop);
            }
        }
    }
    tracing::info!(target: "onde_core::network", "event=tcp_dial_stopped peer={peer_key}");
}

/// Une connexion cliente : moitié écriture sur un fil dédié (queue sortante
/// du pair), moitié lecture inline. Au retour de la lecture, la socket est
/// fermée → l'écrivain débloque en erreur et ré-enfile la frame non partie.
fn run_client_connection(stream: TcpStream, peer_key: String, shared: &Arc<Shared>) {
    configure_stream(&stream, shared.config.socket_timeout);
    let writer_stream = match stream.try_clone() {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(target: "onde_core::network", "event=tcp_try_clone_failed peer={peer_key} error={e}");
            None
        }
    };
    if let Some(ws) = writer_stream {
        let s = shared.clone();
        let key = peer_key.clone();
        // Fil d'écriture détaché : il meurt seul quand la socket tombe
        // (erreur d'écriture prompte après notre shutdown en fin de lecture).
        let spawned = std::thread::Builder::new()
            .name("onde-tcp-write".to_string())
            .spawn(move || writer_loop(ws, key, s));
        if spawned.is_err() {
            tracing::warn!(target: "onde_core::network", "event=tcp_writer_thread_spawn_failed peer={peer_key}");
        }
    }

    read_frames_until_closed(stream, peer_key, shared);
    // La lecture est terminée : le shutdown(BOTH) de
    // `read_frames_until_closed` a déjà débloqué l'écrivain (erreur
    // d'écriture prompte). Rien d'autre à joindre — fils détachés.
}

/// Sommeil interrompable par tranches courtes : l'arrêt est pris en compte
/// au plus tard après `slice`, jamais au-delà de l'intervalle demandé.
fn sleep_bounded(total: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(50);
    let mut remaining = total;
    while remaining > Duration::ZERO && !stop.load(Ordering::Acquire) {
        let step = remaining.min(slice);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

/// Fil d'écriture d'une connexion cliente : défile la queue sortante DU PAIR
/// et écrit chaque frame. Erreur d'écriture → re-file la frame en tête
/// (elle partira à la prochaine connexion réussie) puis fin du fil.
fn writer_loop(mut stream: TcpStream, peer_key: String, shared: Arc<Shared>) {
    configure_stream(&stream, shared.config.socket_timeout);
    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let next = {
            let mut map = lock_or_recover(&shared.outbound);
            match map.get_mut(&peer_key) {
                Some(queue) => queue.pop_front(),
                None => None,
            }
        };
        match next {
            None => {
                std::thread::sleep(WRITER_POLL_INTERVAL);
            }
            Some(payload) => match encode_frame(&payload) {
                Err(e) => {
                    // Inatteignable en pratique (validé à l'enfilement) ;
                    // frame perdue volontairement, comptée, jamais paniqué.
                    tracing::error!(target: "onde_core::network", "event=tcp_encode_failed peer={peer_key} error={e}");
                }
                Ok(frame) => match stream.write_all(&frame) {
                    Ok(()) => {
                        shared.stats.frames_sent.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        // Re-file en tête : la frame partira à la prochaine
                        // connexion (le receveur coupera cette socket corrompue).
                        lock_or_recover(&shared.outbound)
                            .entry(peer_key.clone())
                            .or_default()
                            .push_front(payload);
                        tracing::debug!(target: "onde_core::network", "event=tcp_write_failed peer={peer_key} error={e}");
                        break;
                    }
                },
            },
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

// ────────────────────────────────────────────────────────────────────────
// Intégration Node — pump gossip ⇆ TCP
// ────────────────────────────────────────────────────────────────────────

/// Taille max d'un lot gossip tiré par pair à chaque passe du pump (même
/// philosophie que [`crate::node::Node::take_heal_batch`] : lots bornés,
/// le pump boucle — jamais de tempête).
pub const PUMP_BATCH_MAX_EVENTS: usize = 64;

/// Bilan d'une passe de pump (observabilité test/CI).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PumpReport {
    /// Frames TCP reçues et traitées cette passe.
    pub frames_received: usize,
    /// Événements admis puis traités avec succès (stockés/appliqués).
    pub events_ingested: u64,
    /// Événements refusés par le gate anti-abus.
    pub events_rejected: u64,
    /// Événements neutres (doublon connu, kind non géré…).
    pub events_neutral: u64,
    /// Frames indécodable au format wire (`from_wire_bytes` en erreur).
    pub decode_errors: usize,
    /// Frames mises en file sortante vers les pairs cette passe.
    pub frames_queued_outbound: usize,
}

/// Instant unix secs courant (même convention que le reste du nœud :
/// `unwrap_or_default` — horloge avant 1970 = 0, jamais de panique).
pub fn unix_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Traiter TOUT ce qui est arrivé par le réseau : pour chaque frame,
/// EXACTEMENT le chemin d'un événement reçu d'un pair gossip —
/// `MeshEvent::from_wire_bytes` → gate d'admission
/// ([`crate::node::Node::receive_peer_event`]) qui re-vérifie signature +
/// PoW + réputation, route vers le handler métier (alerte → stockage tier
/// Critical + SQLite), alimente métriques et réputation.
///
/// Aucun court-circuit : une frame TCP n'a AUCUN privilège par rapport à un
/// événement gossip — c'est la propriété de sécurité clé de l'intégration.
pub fn process_inbound(node: &mut Node, transport: &TcpTransport) -> PumpReport {
    use crate::node::{PeerEventOutcome as Outcome, SocialEventOutcome};
    let mut report = PumpReport::default();
    // T32-C : première raison de refus de la passe (paire, raison) — les
    // refus d'admission ne laissent AUCUNE autre trace hors compteurs
    // mémoire ; sur appareils (incident 2026-08-24), un refus légitime du
    // gate anti-abus était indistinguable d'une perte réseau (« silence
    // total »). Une ligne structurée par passe rend le verdict visible sans
    // inonder les logs (au pire une ligne par passe de pump, pas par event).
    let mut first_rejection: Option<(String, String)> = None;
    for frame in transport.drain_inbound() {
        report.frames_received += 1;
        match MeshEvent::from_wire_bytes(&frame.payload) {
            Ok(event) => {
                let outcome = node.receive_peer_event(unix_now_secs(), &event);
                match outcome {
                    Outcome::AlertStored
                    | Outcome::EndorsementApplied
                    | Outcome::AbuseReportApplied(_) => report.events_ingested += 1,
                    Outcome::Social(so) => match so {
                        SocialEventOutcome::PostStored(_)
                        | SocialEventOutcome::CommentStored(_)
                        | SocialEventOutcome::VoteApplied
                        | SocialEventOutcome::FollowApplied
                        | SocialEventOutcome::MessageStored
                        | SocialEventOutcome::ModerationApplied => report.events_ingested += 1,
                        SocialEventOutcome::Ignored => report.events_neutral += 1,
                    },
                    Outcome::Rejected(reason)
                    | Outcome::EndorsementRejected(reason)
                    | Outcome::AbuseReportRejected(reason) => {
                        if first_rejection.is_none() {
                            first_rejection = Some((frame.peer_key.clone(), reason));
                        }
                        report.events_rejected += 1;
                    }
                    // Alerte valide non retenue (déjà connue/budget), kind non
                    // géré : neutre — ni ingéré ni rejeté (convention Node).
                    Outcome::AlertNotStored | Outcome::Other => report.events_neutral += 1,
                }
            }
            Err(e) => {
                report.decode_errors += 1;
                tracing::warn!(target: "onde_core::network", "event=tcp_wire_decode_failed peer={} error={e}", frame.peer_key);
            }
        }
    }
    if let Some((peer_key, reason)) = first_rejection {
        tracing::warn!(target: "onde_core::network",
            "event=tcp_admission_rejected count={} first_peer={peer_key} first_reason={reason}",
            report.events_rejected);
    }
    report
}

/// Pousser vers le réseau ce que le gossip doit livrer : pour chaque pair
/// configuré, tirer un lot borné d'événements pending
/// ([`GossipProtocol::peek_pending_for_peer`], SANS marquage), les encoder en
/// wire padé, les mettre en file sortante — et ne marquer « livré » QUE les
/// événements effectivement enfilés (même sémantique que
/// [`crate::node::Node::take_heal_batch`] : marquage après sélection réussie).
pub fn flush_outbound(node: &mut Node, transport: &TcpTransport) -> PumpReport {
    let mut report = PumpReport::default();
    for peer_key in transport.peer_keys() {
        let batch = node
            .gossip
            .peek_pending_for_peer(&peer_key, PUMP_BATCH_MAX_EVENTS);
        if batch.is_empty() {
            continue;
        }
        let mut delivered_ids: Vec<String> = Vec::with_capacity(batch.len());
        for event in &batch {
            match event.to_wire_bytes() {
                Ok(bytes) => match transport.enqueue_outbound(&peer_key, bytes) {
                    Ok(()) => {
                        delivered_ids.push(event.id.clone());
                        report.frames_queued_outbound += 1;
                    }
                    Err(e) => {
                        // Queue pleine ou payload hors borne : on laisse NON
                        // marqué → nouvelle tentative à la prochaine passe.
                        tracing::warn!(target: "onde_core::network", "event=tcp_enqueue_failed peer={peer_key} event={} error={e}", event.id);
                        break;
                    }
                },
                Err(e) => {
                    // Encodage impossible (identité hex invalide) : échec
                    // PERMANENT → marqué livré quand même, sinon la même
                    // frame serait retentée à l'infini à chaque passe.
                    delivered_ids.push(event.id.clone());
                    tracing::error!(target: "onde_core::network", "event=tcp_wire_encode_failed peer={peer_key} event={} error={e}", event.id);
                }
            }
        }
        node.gossip
            .mark_delivered_to_peer(&peer_key, &delivered_ids);
    }
    report
}

// ────────────────────────────────────────────────────────────────────────
// Tests — framing + bornes de queues (contrat, chemins d'erreur)
// ────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Le wire padé d'un petit événement tient dans le seau minimal ;
    /// on construit un payload réaliste sans dépendre du protocole.
    fn sample_payload(len: usize) -> Vec<u8> {
        let mut v = vec![0xA5u8; len];
        v[0] = 0x01; // jamais tout-zéro : représentatif d'un vrai wire
        v
    }

    #[test]
    fn frame_roundtrip_exact_payload() {
        for len in [1usize, 32, 255, 256, 1024, 4096, MAX_FRAME_PAYLOAD] {
            let payload = sample_payload(len);
            let frame = encode_frame(&payload).expect("payload within bounds must encode");
            assert_eq!(frame.len(), FRAME_HEADER_LEN + len);

            let mut reader = FrameReader::new();
            let mut cursor = Cursor::new(frame);
            match reader.poll(&mut cursor) {
                FramePoll::Frame(got) => assert_eq!(got, payload),
                other => panic!("expected a complete frame, got {other:?}"),
            }
            assert_eq!(reader.poll(&mut cursor), FramePoll::Eof);
        }
    }

    #[test]
    fn multiple_frames_sequential_parse() {
        let p1 = sample_payload(10);
        let p2 = sample_payload(700);
        let mut stream_bytes = encode_frame(&p1).expect("encode p1");
        stream_bytes.extend_from_slice(&encode_frame(&p2).expect("encode p2"));

        let mut reader = FrameReader::new();
        let mut cursor = Cursor::new(stream_bytes);
        assert_eq!(reader.poll(&mut cursor), FramePoll::Frame(p1));
        assert_eq!(reader.poll(&mut cursor), FramePoll::Frame(p2));
        assert_eq!(reader.poll(&mut cursor), FramePoll::Eof);
    }

    #[test]
    fn incremental_reads_reconstruct_frame() {
        // Simule une socket qui livre les octets par petites tranches :
        // le lecteur doit rendre Pending puis reconstituer la frame.
        // Lecteur qui livre UNE tranche par appel puis WouldBlock — la
        // sémantique exacte d'une socket en mode timeout : « pas encore de
        // données » n'est PAS une fin de flux.
        struct TrickleReader {
            chunks: Vec<Vec<u8>>,
            next: usize,
            offset: usize,
        }
        impl Read for TrickleReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                // Livre la tranche courante (avec curseur si le lecteur
                // n'a demandé qu'une partie), puis WouldBlock une fois
                // tout le flux consommé.
                if self.next >= self.chunks.len() || buf.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "no data yet",
                    ));
                }
                let chunk = &self.chunks[self.next];
                let start = self.offset;
                let n = (chunk.len() - start).min(buf.len());
                buf[..n].copy_from_slice(&chunk[start..start + n]);
                self.offset += n;
                if self.offset == chunk.len() {
                    self.next += 1;
                    self.offset = 0;
                }
                Ok(n)
            }
        }

        let payload = sample_payload(300);
        let frame = encode_frame(&payload).expect("encode");
        let mut trickle = TrickleReader {
            chunks: frame.chunks(7).map(|c| c.to_vec()).collect(),
            next: 0,
            offset: 0,
        };
        let mut reader = FrameReader::new();
        // Une passe de poll par tranche disponible ; chaque passe doit soit
        // rendre Pending (flux interrompu par WouldBlock), soit livrer la
        // frame complète à la dernière tranche.
        for _ in 0..trickle.chunks.len() {
            match reader.poll(&mut trickle) {
                FramePoll::Pending => {} // entre deux tranches : normal
                FramePoll::Frame(got) => {
                    assert_eq!(got, payload);
                    return;
                }
                other => panic!("unexpected poll result mid-stream: {other:?}"),
            }
        }
        panic!("frame never completed across chunks");
    }

    #[test]
    fn clean_eof_between_frames_is_not_an_error() {
        let mut reader = FrameReader::new();
        let mut empty = Cursor::new(Vec::<u8>::new());
        assert_eq!(reader.poll(&mut empty), FramePoll::Eof);
    }

    #[test]
    fn truncated_header_is_fatal() {
        let frame = encode_frame(&sample_payload(64)).expect("encode");
        let truncated = &frame[..3]; // en-tête incomplet
        let mut reader = FrameReader::new();
        let mut cursor = Cursor::new(truncated.to_vec());
        match reader.poll(&mut cursor) {
            FramePoll::Fatal(reason) => {
                assert!(reason.contains("truncated"), "reason: {reason}");
            }
            other => panic!("truncated header must be Fatal, got {other:?}"),
        }
    }

    #[test]
    fn truncated_payload_is_fatal() {
        let frame = encode_frame(&sample_payload(100)).expect("encode");
        let truncated = &frame[..FRAME_HEADER_LEN + 40]; // corps coupé
        let mut reader = FrameReader::new();
        let mut cursor = Cursor::new(truncated.to_vec());
        match reader.poll(&mut cursor) {
            FramePoll::Fatal(reason) => {
                assert!(reason.contains("truncated"), "reason: {reason}");
            }
            other => panic!("truncated payload must be Fatal, got {other:?}"),
        }
    }

    #[test]
    fn oversized_declared_length_refused_without_reading_body() {
        // En-tête déclarant MAX+1 : refus immédiat, AVANT toute lecture du
        // corps — le corps n'est même pas présent dans le flux.
        let mut malicious = Vec::new();
        malicious.extend_from_slice(&(MAX_FRAME_PAYLOAD as u32 + 1).to_be_bytes());
        let mut reader = FrameReader::new();
        let mut cursor = Cursor::new(malicious);
        match reader.poll(&mut cursor) {
            FramePoll::Fatal(reason) => {
                assert!(reason.contains("too large"), "reason: {reason}");
            }
            other => panic!("oversized frame must be refused, got {other:?}"),
        }
    }

    #[test]
    fn zero_length_frame_is_a_protocol_violation() {
        let mut reader = FrameReader::new();
        let mut cursor = Cursor::new(0u32.to_be_bytes().to_vec());
        match reader.poll(&mut cursor) {
            FramePoll::Fatal(reason) => {
                assert!(reason.contains("zero length"), "reason: {reason}");
            }
            other => panic!("zero-length frame must be Fatal, got {other:?}"),
        }
    }

    #[test]
    fn encode_frame_rejects_oversize_payload_cleanly() {
        let too_big = sample_payload(MAX_FRAME_PAYLOAD + 1);
        let err = encode_frame(&too_big).expect_err("oversize payload must be refused");
        assert!(err.contains("too large"), "error: {err}");
        // La borne exacte reste acceptée.
        assert!(encode_frame(&sample_payload(MAX_FRAME_PAYLOAD)).is_ok());
    }

    /// Transport de test : config minimale (pas de socket réelle).
    fn test_transport(capacity: usize) -> TcpTransport {
        TcpTransport::new(TcpTransportConfig {
            queue_capacity: capacity,
            ..TcpTransportConfig::default()
        })
    }

    #[test]
    fn outbound_queue_is_bounded_and_evicts_oldest() {
        let transport = test_transport(4);
        for i in 0..6u8 {
            transport
                .enqueue_outbound("peer-a", vec![i; 8])
                .expect("within bounds");
        }
        let stats = transport.stats();
        assert_eq!(
            stats.frames_dropped_outbound_full, 2,
            "2 frames au-delà de la capacité doivent être évincées"
        );
        // Éviction FIFO : les frames 0 et 1 sont parties, les 2..=5 restent
        // en tête de queue pour le prochain écrivain. La queue sortante est
        // interne au transport ; le compteur dédié est le contrat observable.
    }

    #[test]
    fn enqueue_outbound_rejects_oversize_payload() {
        let transport = test_transport(8);
        let err = transport
            .enqueue_outbound("peer-a", sample_payload(MAX_FRAME_PAYLOAD + 1))
            .expect_err("oversize must be refused at enqueue time");
        assert!(err.contains("too large"), "error: {err}");
    }
}
