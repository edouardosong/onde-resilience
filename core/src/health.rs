//! Phase 3.6 — Endpoint de santé HTTP **localhost uniquement**.
//!
//! Serveur HTTP minimaliste fondé sur `std::net::TcpListener` (AUCUN
//! framework HTTP, aucune dépendance nouvelle). Désactivé par défaut :
//! il n'existe que si l'appelant ([`crate::bin`] `onde_node --health-port N`)
//! le démarre explicitement.
//!
//! Garanties :
//! - écoute exclusivement sur `127.0.0.1` (jamais d'interface externe) ;
//! - budget de temps **total** par connexion ([`CONNECTION_BUDGET`], appliqué
//!   aux lectures) EN PLUS des timeouts socket par opération — un client lent
//!   type slowloris ne peut retenir un thread au-delà du budget global ;
//! - plafond de connexions concurrentes : la décision d'admission est prise
//!   **atomiquement dans la boucle d'acceptation, avant tout spawn**, donc
//!   une rafale ne peut pas dépasser le plafond ; au-delà → réponse 503
//!   immédiate servie inline (zéro thread spawned) ;
//! - taille de requête plafonnée (8 Kio) : au-delà → réponse **431** PUIS
//!   fermeture (la doc promettait déjà 431 — l'implémentation s'y aligne) ;
//! - arrêt propre VÉRIFIABLE : le thread d'acceptation possède le SEUL
//!   listener (aucun clone partagé, fd unique) en mode non-bloquant et sonde
//!   le fanion d'arrêt à intervalle court ([`ACCEPT_POLL_INTERVAL`]) ; à
//!   l'arrêt il sort de sa boucle, ce qui ferme le listener et libère le
//!   port — y compris sans aucune connexion entrante (un rebind immédiat
//!   sur le même port réussit).
//!
//! # Exemple (port éphémère, CI-safe)
//!
//! ```
//! use std::io::{Read, Write};
//! use std::net::TcpStream;
//! use std::sync::Arc;
//! use onde_core::health::spawn_health_server;
//! use onde_core::metrics::NodeMetrics;
//!
//! let metrics = Arc::new(NodeMetrics::new());
//! // Port 0 = bind éphémère ; le port effectif est renvoyé dans la poignée.
//! let handle = spawn_health_server(0, metrics).expect("bind localhost");
//! assert_ne!(handle.port, 0);
//!
//! let mut stream = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
//! stream
//!     .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
//!     .unwrap();
//! let mut raw = String::new();
//! stream.read_to_string(&mut raw).unwrap();
//! assert!(raw.starts_with("HTTP/1.1 200 OK"));
//! assert!(raw.contains("\"status\":\"ok\""));
//! drop(handle); // arrêt propre du thread d'acceptation
//! ```
use std::io::{Read, Write};
use std::net::{Shutdown as SocketShutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::metrics::NodeMetrics;

/// Taille maximale acceptée pour une requête (octets) — au-delà → 431.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// Timeout borné appliqué à chaque socket client (lecture ET écriture) —
/// borne PAR OPÉRATION, complétée par le budget total ci-dessous.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// Budget de temps **TOTAL** par connexion (lectures cumulées) : un client
/// qui drippe ses octets assez vite pour éviter chaque timeout individuel
/// (slowloris) est coupé dès que ce budget global est épuisé. Aucune
/// connexion ne peut retenir un thread au-delà.
const CONNECTION_BUDGET: Duration = Duration::from_secs(10);

/// Intervalle de sondage du fanion d'arrêt par la boucle d'acceptation
/// (listener non-bloquant) : latence d'arrêt bornée par cette valeur.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Plafond de threads de connexion actifs ; au-delà, réponse 503 servie en
/// ligne par la boucle d'acceptation (jamais de spawn illimité).
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// Nombre d'erreurs consécutives d'acceptation tolérées avant arrêt propre
/// (le listener est moribond → on rend la main plutôt que de tourner à vide).
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 32;

/// Poignée du serveur de santé — port effectif + arrêt propre.
///
/// Le thread d'acceptation détient l'UNIQUE listener (aucun clone `try_clone`
/// partagé : dropper un clone ne ferme pas le socket et ne réveille pas un
/// `accept()` bloquant). Cette poignée n'a donc AUCUN descripteur à fermer :
/// la destruction (`Drop`) ou [`HealthHandle::shutdown`] arme seulement le
/// fanion d'arrêt. Le thread, dont le listener est non-bloquant, sonde ce
/// fanion au plus tard après [`ACCEPT_POLL_INTERVAL`], quitte sa boucle, et
/// c'est LA SORTIE DU THREAD qui ferme l'unique fd — libérant réellement le
/// port, sans nécessiter la moindre connexion entrante pour « pomper » la
/// boucle.
#[derive(Debug)]
pub struct HealthHandle {
    /// Port effectif lié (utile après un bind sur le port 0 éphémère).
    pub port: u16,
    stop_flag: Arc<AtomicBool>,
}

impl HealthHandle {
    /// Arrêt propre : arme le fanion d'arrêt ; le thread d'acceptation sort
    /// de sa boucle sous [`ACCEPT_POLL_INTERVAL`], ferme le listener (port
    /// libéré) et se termine. Les connexions déjà acceptées s'achèvent
    /// seules, au plus tard au bout du budget total par connexion.
    pub fn shutdown(self) {
        // L'armement vit dans `Drop` (chemin unique, idempotent) : cette
        // méthode nommée exprime l'intention sur les sites d'appel.
        drop(self);
    }
}

impl Drop for HealthHandle {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
    }
}

/// Démarre le serveur de santé lié à `127.0.0.1:port`.
///
/// `port = 0` → bind éphémère choisi par l'OS ; le port effectif est exposé
/// par [`HealthHandle::port`]. Le thread d'acceptation est détaché (le nœud
/// n'a pas à attendre sa fin) mais reste propre : erreurs persistantes ou
/// signal d'arrêt → sortie nette avec log structuré.
///
/// # Erreurs
///
/// Retourne l'erreur `std::io` du bind (port occupé, privilèges…) —
/// l'appelant décide : fatal pour un daemon demandant explicitement la
/// santé, jamais silencieux.
pub fn spawn_health_server(port: u16, metrics: Arc<NodeMetrics>) -> std::io::Result<HealthHandle> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let effective_port = listener.local_addr()?.port();
    // Le thread devient l'unique propriétaire du listener, passé en
    // non-bloquant : `accept()` ne bloque jamais, la boucle re-sonde le
    // fanion d'arrêt à intervalle court, et la sortie du thread ferme le
    // seul fd → libération du port vérifiable (rebind immédiat).
    listener.set_nonblocking(true)?;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_thread = stop_flag.clone();
    let active = Arc::new(AtomicUsize::new(0));

    std::thread::Builder::new()
        .name("onde-health".to_string())
        .spawn(move || accept_loop(listener, metrics, active, stop_thread))?;

    Ok(HealthHandle {
        port: effective_port,
        stop_flag,
    })
}

/// Boucle d'acceptation : sert chaque connexion (thread dédié plafonné),
/// s'arrête proprement sur signal d'arrêt, erreur persistante ou listener
/// fermé.
fn accept_loop(
    listener: TcpListener,
    metrics: Arc<NodeMetrics>,
    active: Arc<AtomicUsize>,
    stop_flag: Arc<AtomicBool>,
) {
    let mut consecutive_errors: u32 = 0;
    loop {
        if stop_flag.load(Ordering::Acquire) {
            break clean_stop();
        }
        match listener.accept() {
            Ok((stream, _peer)) => {
                consecutive_errors = 0;
                // Admission DANS LE PARENT, atomique et AVANT tout spawn :
                // le compteur est incrémenté au moment de la décision, donc
                // une rafale ne peut pas dépasser le plafond (pas de fenêtre
                // TOCTOU entre la décision et l'incrément côté thread enfant).
                if active.fetch_add(1, Ordering::AcqRel) >= MAX_CONCURRENT_CONNECTIONS {
                    active.fetch_sub(1, Ordering::Release);
                    // Surchargé : refus explicite servi inline par la boucle
                    // d'acceptation elle-même, aucun thread spawned.
                    serve_inline_busy(stream);
                    continue;
                }
                let m = metrics.clone();
                let a = active.clone();
                let spawned = std::thread::Builder::new()
                    .name("onde-health-conn".to_string())
                    .spawn(move || {
                        handle_connection(stream, &m);
                        a.fetch_sub(1, Ordering::Release);
                    });
                if spawned.is_err() {
                    // Épuisement de threads OS : on ignore cette connexion
                    // (le client retentera) sans jamais paniquer ; le quota
                    // pris à l'admission est rendu immédiatement.
                    active.fetch_sub(1, Ordering::Release);
                    continue;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                // Pas de connexion prête (listener non-bloquant) : courte
                // attente, puis re-sonde du fanion d'arrêt en tête de boucle.
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) => {
                if stop_flag.load(Ordering::Acquire) {
                    break clean_stop();
                }
                consecutive_errors += 1;
                tracing::warn!("event=health_accept_error count={consecutive_errors} error={e}");
                if consecutive_errors >= MAX_CONSECUTIVE_ACCEPT_ERRORS {
                    tracing::error!(
                        "event=health_listener_died consecutive_errors={consecutive_errors}"
                    );
                    return;
                }
                std::thread::sleep(ACCEPT_POLL_INTERVAL);
            }
        }
    }
}

/// Arrêt propre de la boucle d'acceptation (log structuré unique).
fn clean_stop() {
    tracing::info!("event=health_stopped reason=shutdown");
}

/// Réponse 503 servie inline quand le plafond de connexions est atteint.
///
/// La fermeture passe par `shutdown(BOTH)` AVANT le drop : fermer un socket
/// dont le tampon de réception contient encore la requête du client émettrait
/// un RST (le client verrait « Connection reset » au lieu de notre 503).
fn serve_inline_busy(mut stream: TcpStream) {
    let body = "{\"error\":\"busy\"}";
    let _ = write_response(&mut stream, "503 Service Unavailable", body);
    let _ = stream.shutdown(SocketShutdown::Both);
}

/// Traite UNE connexion avec le budget total par défaut.
fn handle_connection(stream: TcpStream, metrics: &Arc<NodeMetrics>) {
    handle_connection_with_budget(stream, metrics, CONNECTION_BUDGET);
}

/// Traite UNE connexion avec un budget de temps **TOTAL** (lectures cumulées)
/// : parse la ligne de requête, route, répond, ferme. Un client qui drippe
/// ses octets assez vite pour rester sous chaque timeout individuel est
/// coupé dès que le budget global est épuisé — aucune connexion ne peut
/// retenir un thread indéfiniment.
fn handle_connection_with_budget(
    mut stream: TcpStream,
    metrics: &Arc<NodeMetrics>,
    budget: Duration,
) {
    let deadline = Instant::now() + budget;
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));

    let request_line = match read_request_head(&mut stream, deadline) {
        RequestHead::Line(line) => line,
        RequestHead::TooLarge => {
            // Aligné sur la documentation : répondre 431 PUIS fermer, au
            // lieu d'une fermeture silencieuse (RST observé avant T23).
            let _ = write_response(
                &mut stream,
                "431 Request Header Fields Too Large",
                "{\"error\":\"request too large\"}",
            );
            let _ = stream.shutdown(SocketShutdown::Both);
            return;
        }
        RequestHead::Dead => return, // budget épuisé, timeout, connexion vide ou fermée
    };

    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let is_get = method == "GET";
    let is_health_path = path == "/health" || path == "/health?";

    let (status, body) = if !is_get && path == "/health" {
        (
            "405 Method Not Allowed",
            "{\"error\":\"method not allowed\"}".to_string(),
        )
    } else if is_get && is_health_path {
        metrics.record_health_request();
        let body = metrics.snapshot_json();
        tracing::debug!(
            "event=health_request path=/health status=200 count={}",
            body.len()
        );
        ("200 OK", body)
    } else if is_get {
        ("404 Not Found", "{\"error\":\"not found\"}".to_string())
    } else {
        (
            "405 Method Not Allowed",
            "{\"error\":\"method not allowed\"}".to_string(),
        )
    };
    let _ = write_response(&mut stream, status, &body);
    let _ = stream.shutdown(SocketShutdown::Both);
}

/// Issue de la lecture de la tête de requête.
enum RequestHead {
    /// Ligne de requête complète (première ligne non vide terminée par CRLF).
    Line(String),
    /// Plafond de taille dépassé sans fin de ligne → le serveur répondra 431.
    TooLarge,
    /// Rien d'exploitable : budget total épuisé, timeout par opération,
    /// connexion vide ou fermée prématurément → fermeture sans réponse.
    Dead,
}

/// Lit l'en-tête HTTP jusqu'à la première ligne complète (CRLF), avec :
/// - un plafond strict [`MAX_REQUEST_BYTES`] — au-delà SANS fin de ligne →
///   [`RequestHead::TooLarge`] (réponse 431 puis fermeture) ;
/// - un budget TOTAL `deadline` (slowloris) : chaque lecture individuelle
///   est bornée par min([`SOCKET_TIMEOUT`], temps restant), et la boucle
///   s'arrête dès que l'échéance globale est atteinte ;
/// - la tolérance RFC 7230 §3.5 : les lignes vides initiales sont ignorées.
fn read_request_head(stream: &mut TcpStream, deadline: Instant) -> RequestHead {
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return RequestHead::Dead; // budget total épuisé → coupure slowloris
        }
        // Timeout par opération = min(timeout socket, temps restant) :
        // même sans vérification explicite, aucun read ne déborde du budget.
        let _ = stream.set_read_timeout(Some(SOCKET_TIMEOUT.min(deadline - now)));
        match stream.read(&mut chunk) {
            Ok(0) => return RequestHead::Dead, // fermeture sans requête complète
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // RFC 7230 §3.5 : ignorer d'éventuelles lignes vides initiales
                // (certains clients envoient un CRLF supplémentaire).
                loop {
                    match find_crlf(&buf) {
                        Some(0) => {
                            buf.drain(..2);
                        }
                        Some(pos) => {
                            return RequestHead::Line(
                                String::from_utf8_lossy(&buf[..pos]).into_owned(),
                            );
                        }
                        None => break,
                    }
                }
                if buf.len() > MAX_REQUEST_BYTES {
                    return RequestHead::TooLarge;
                }
            }
            Err(_) => return RequestHead::Dead, // timeout lecture ou erreur socket
        }
    }
}

/// Position du premier `\r\n` dans `buf`, si présent.
fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

/// Écrit une réponse HTTP/1.1 minimale JSON + `Connection: close`.
fn write_response(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Preuve de libération par REBIND direct sur le MÊME port : un ancien
    /// test sondait avec des connexions TCP, ce qui « pompait » la boucle
    /// d'acceptation et masquait l'arrêt cassé (le clone du listener ne
    /// fermait pas le socket). Ici, aucune sonde : seul le fanion d'arrêt
    /// sondé par le thread doit fermer l'unique fd, sous délai borné.
    fn assert_port_rebindable(port: u16) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            match TcpListener::bind(("127.0.0.1", port)) {
                Ok(rebound) => {
                    drop(rebound);
                    return;
                }
                Err(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "port must be released (rebindable) after shutdown"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    }

    /// Paire client/serveur locale pour tester les fonctions de service
    /// directement (budget court, sans passer par le thread d'acceptation).
    fn local_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    /// Client HTTP minimal (std-only) : GET, lit toute la réponse.
    fn http_get(port: u16, target: &str, method: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("server must be reachable");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let req =
            format!("{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).expect("response must end");
        raw
    }

    #[test]
    fn test_health_endpoint_serves_parseable_json_with_all_fields() {
        let metrics = Arc::new(NodeMetrics::new());
        metrics.record_ingested();
        metrics.set_peers(3, 2);
        let handle = spawn_health_server(0, metrics.clone()).expect("ephemeral bind");
        assert_ne!(handle.port, 0, "port 0 must resolve to the effective port");

        let raw = http_get(handle.port, "/health", "GET");
        assert!(raw.starts_with("HTTP/1.1 200 OK"), "got: {raw}");
        assert!(raw
            .to_ascii_lowercase()
            .contains("content-type: application/json"));

        let body_start = raw.find("\r\n\r\n").expect("headers/body separator") + 4;
        let v: Value =
            serde_json::from_str(raw[body_start..].trim()).expect("body must be valid JSON");
        assert_eq!(v["status"], "ok");
        // Assertion numérique SAINe (pas seulement is_u64) : l'uptime d'un
        // serveur fraîchement démarré est borné par quelques secondes — un
        // unix_now cassé (→ 0 ou horloge absurde) sort de cette borne.
        let uptime = v["uptime_s"].as_u64().expect("uptime_s numeric");
        assert!(
            uptime <= 60,
            "fresh server uptime must be small, got {uptime}"
        );
        assert_eq!(v["peers"]["known"], 3);
        assert_eq!(v["peers"]["synced"], 2);
        for field in [
            "messages_ingested",
            "messages_rejected",
            "messages_gossiped",
            "messages_duplicated",
            "health_requests",
        ] {
            assert!(v["metrics"][field].is_u64(), "missing metrics.{field}: {v}");
        }
        assert_eq!(v["metrics"]["messages_ingested"], 1);
        assert!(v["storage"]["events"].is_u64());
        assert!(v["storage"]["bytes_raw"].is_u64());
        assert!(v["storage"]["bytes_stored"].is_u64());
        handle.shutdown();
    }

    #[test]
    fn test_unknown_path_is_404_and_wrong_method_405() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");

        let raw404 = http_get(handle.port, "/nope", "GET");
        assert!(raw404.starts_with("HTTP/1.1 404"), "got: {raw404}");

        let raw405 = http_get(handle.port, "/health", "POST");
        assert!(raw405.starts_with("HTTP/1.1 405"), "got: {raw405}");

        // La requête POST ne doit pas avoir incrémenté le compteur /health.
        let raw = http_get(handle.port, "/health", "GET");
        let body_start = raw.find("\r\n\r\n").unwrap() + 4;
        let v: Value = serde_json::from_str(raw[body_start..].trim()).unwrap();
        assert_eq!(v["metrics"]["health_requests"], 1);
        handle.shutdown();
    }

    #[test]
    fn test_garbage_request_does_not_crash_and_server_keeps_serving() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");

        // Requête binaire absurde : pas de CRLF → ignorée après timeout court
        // côté serveur ; on borne le test en fermant tout de suite.
        let mut s = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        s.write_all(&[0xff, 0x00, 0xfe]).unwrap();
        drop(s);

        // Le serveur doit continuer à répondre normalement.
        let raw = http_get(handle.port, "/health", "GET");
        assert!(raw.starts_with("HTTP/1.1 200 OK"), "got: {raw}");
        handle.shutdown();
    }

    #[test]
    fn test_shutdown_releases_port_without_probes() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let port = handle.port;
        handle.shutdown();
        // AUCUNE connexion sonde entre shutdown et vérification : la seule
        // force de libération est le fanion d'arrêt sondé par le thread
        // (≤ ACCEPT_POLL_INTERVAL). Une petite attente couvre plusieurs polls.
        std::thread::sleep(Duration::from_millis(150));
        assert_port_rebindable(port);
    }

    #[test]
    fn test_drop_impl_also_stops_server_without_probes() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let port = handle.port;
        drop(handle);
        std::thread::sleep(Duration::from_millis(150));
        assert_port_rebindable(port);
    }

    #[test]
    fn test_burst_beyond_cap_admits_exactly_16_then_busy_cleanly() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");

        // 16 titulaires : requête INCOMPLÈTE (pas de CRLF) → leur thread de
        // connexion reste actif, bloqué en lecture dans les limites du budget.
        let mut holders = Vec::new();
        for _ in 0..MAX_CONCURRENT_CONNECTIONS {
            let mut s = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
            let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
            s.write_all(b"GET /he").expect("holder write");
            holders.push(s);
        }
        // Laisse la boucle parente admettre les 16 (fetch_add côté parent).
        std::thread::sleep(Duration::from_millis(200));

        // Toute nouvelle connexion pendant que les 16 sont actives → 503 busy
        // PROPRE (réponse complète lue, corps exact) — tue le mutant
        // `>=`→`<` (qui refuserait au contraire SOUS le plafond) et le mutant
        // serve_inline_busy vidé (réponse absente).
        let late = http_get(handle.port, "/health", "GET");
        assert!(late.starts_with("HTTP/1.1 503"), "got: {late}");
        assert!(late.contains("{\"error\":\"busy\"}"), "got: {late}");

        // Rafale supplémentaire : toutes servies inline busy, zéro crash.
        for _ in 0..9 {
            let raw = http_get(handle.port, "/health", "GET");
            assert!(raw.starts_with("HTTP/1.1 503"), "got: {raw}");
        }

        // Libération : les titulaires ferment → capacité restaurée → 200.
        drop(holders);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let raw = http_get(handle.port, "/health", "GET");
            if raw.starts_with("HTTP/1.1 200") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "capacity must be restored after holders close"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        handle.shutdown();
    }

    #[test]
    fn test_oversized_request_gets_431_then_close() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let mut s = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        s.write_all(&vec![b'A'; 20 * 1024])
            .expect("oversized write");
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)
            .expect("server must answer 431 then close");
        let raw = String::from_utf8_lossy(&raw).to_string();
        assert!(raw.starts_with("HTTP/1.1 431"), "expected 431, got: {raw}");
        assert!(
            raw.ends_with("{\"error\":\"request too large\"}"),
            "got: {raw}"
        );
        handle.shutdown();
    }

    #[test]
    fn test_exact_cap_bytes_without_crlf_not_yet_rejected() {
        // Exactement MAX_REQUEST_BYTES octets SANS CRLF : sous le plafond
        // STRICT (`>`), le serveur ne doit PAS encore répondre — il attend la
        // suite. Tue le mutant `>`→`>=` (qui rejetterait à la borne exacte).
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let mut s = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        s.write_all(&vec![b'A'; MAX_REQUEST_BYTES])
            .expect("boundary write");
        let _ = s.set_read_timeout(Some(Duration::from_millis(400)));
        let mut probe = [0u8; 64];
        match s.read(&mut probe) {
            Ok(n) => panic!("server answered prematurely below strict cap ({n} bytes)"),
            Err(e) => assert!(
                matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ),
                "unexpected error: {e}"
            ),
        }
        drop(s);
        handle.shutdown();
    }

    #[test]
    fn test_large_but_legal_request_line_under_cap_succeeds() {
        // ~7 Kio DANS la ligne de requête (espaces inter-tokens, légaux pour
        // split_whitespace), encore SOUS le plafond de 8 Kio → 200 attendu.
        // Tue les mutants de `8 * 1024` : `8 + 1024` (cap réduit à 1032 →
        // 431 prématuré) et `8 / 1024` (cap 0 → 431 immédiat).
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let pad = " ".repeat(7 * 1024);
        let req = format!("GET {pad}/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(req.len() > 8 + 1024, "pad must exceed the *→+ mutant cap");
        assert!(
            req.len() < MAX_REQUEST_BYTES,
            "must stay under the strict cap"
        );
        let mut s = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        s.write_all(req.as_bytes()).expect("legal large write");
        let mut raw = String::new();
        s.read_to_string(&mut raw).expect("response must end");
        assert!(
            raw.starts_with("HTTP/1.1 200"),
            "got: {}",
            &raw[..raw.len().min(40)]
        );
        handle.shutdown();
    }

    #[test]
    fn test_leading_empty_lines_tolerated_rfc7230_3_5() {
        // RFC 7230 §3.5 : un CRLF vide initial doit être ignoré, pas traité
        // comme une request-line vide (ancien comportement → 405).
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let mut s = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        s.write_all(b"\r\n\r\nGET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .expect("leading-crlf write");
        let mut raw = String::new();
        s.read_to_string(&mut raw).expect("response must end");
        assert!(
            raw.starts_with("HTTP/1.1 200"),
            "got: {}",
            &raw[..raw.len().min(40)]
        );
        handle.shutdown();
    }

    #[test]
    fn test_slowloris_cannot_hold_thread_past_total_budget() {
        // Chemin réel partagé : handle_connection_with_budget avec budget
        // court. Le client drippe des octets plus vite que tout timeout
        // individuel → le serveur doit couper dès le budget TOTAL épuisé.
        let (client, server_stream) = local_pair();
        let metrics = Arc::new(NodeMetrics::new());
        let server = std::thread::spawn(move || {
            handle_connection_with_budget(server_stream, &metrics, Duration::from_millis(600));
        });
        let started = std::time::Instant::now();
        let mut s = client;
        let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
        let mut buf = [0u8; 128];
        let mut eof_at: Option<Duration> = None;
        for _ in 0..60 {
            let _ = s.write_all(b"x");
            let _ = s.set_read_timeout(Some(Duration::from_millis(50)));
            match s.read(&mut buf) {
                Ok(0) => {
                    eof_at = Some(started.elapsed());
                    break;
                }
                Ok(_) => panic!("slowloris drip must not produce a response"),
                Err(e) => assert!(
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ),
                    "unexpected client read error: {e}"
                ),
            }
        }
        let eof = eof_at.expect("server must close the slowloris connection");
        assert!(
            eof < Duration::from_millis(1500),
            "total budget must cut the connection promptly (eof after {eof:?})"
        );
        server.join().expect("server thread must finish cleanly");
    }

    #[test]
    fn test_concurrent_requests_all_answered() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = Arc::new(spawn_health_server(0, metrics).expect("ephemeral bind"));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let h = handle.clone();
            handles.push(std::thread::spawn(move || {
                let raw = http_get(h.port, "/health", "GET");
                assert!(raw.starts_with("HTTP/1.1 200 OK"), "got: {raw}");
            }));
        }
        for h in handles {
            h.join().expect("client thread must not panic");
        }
        drop(handle); // arrêt propre via Drop
    }
}
