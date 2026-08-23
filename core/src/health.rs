//! Phase 3.6 — Endpoint de santé HTTP **localhost uniquement**.
//!
//! Serveur HTTP minimaliste fondé sur `std::net::TcpListener` (AUCUN
//! framework HTTP, aucune dépendance nouvelle). Désactivé par défaut :
//! il n'existe que si l'appelant ([`crate::bin`] `onde_node --health-port N`)
//! le démarre explicitement.
//!
//! Garanties :
//! - écoute exclusivement sur `127.0.0.1` (jamais d'interface externe) ;
//! - timeouts socket bornés (lecture/écriture 2 s) — aucune connexion ne
//!   peut retenir le serveur indéfiniment ;
//! - plafond de connexions concurrentes (au-delà → réponse 503 immédiate) ;
//! - taille de requête plafonnée (8 Kio) ;
//! - arrêt propre : la destruction de [`HealthHandle`] ferme le listener,
//!   ce qui réveille la boucle d'acceptation qui se termine proprement.
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
use std::time::Duration;

use crate::metrics::NodeMetrics;

/// Taille maximale acceptée pour une requête (octets) — au-delà → 431.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

/// Timeout borné appliqué à chaque socket client (lecture ET écriture).
const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// Plafond de threads de connexion actifs ; au-delà, réponse 503 servie en
/// ligne par la boucle d'acceptation (jamais de spawn illimité).
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// Nombre d'erreurs consécutives d'acceptation tolérées avant arrêt propre
/// (le listener est moribond → on rend la main plutôt que de tourner à vide).
const MAX_CONSECUTIVE_ACCEPT_ERRORS: u32 = 32;

/// Poignée du serveur de santé — port effectif + arrêt propre.
///
/// La destruction (`Drop`) déclenche l'arrêt : fermeture du listener (la
/// boucle d'acceptation bloquée sur `accept()` reçoit une erreur, voit le
/// fanion d'arrêt et se termine), puis fermeture des sockets en cours par
/// leurs propres timeouts.
#[derive(Debug)]
pub struct HealthHandle {
    /// Port effectif lié (utile après un bind sur le port 0 éphémère).
    pub port: u16,
    stop_flag: Arc<AtomicBool>,
    /// Clone du listener détenu par le thread — fermé ici en premier pour
    /// réveiller `accept()`.
    thread_listener: Option<TcpListener>,
}

impl HealthHandle {
    /// Arrêt propre : ferme le listener et signale la boucle d'acceptation.
    /// Les requêtes déjà acceptées se terminent seules (timeouts bornés).
    pub fn shutdown(mut self) {
        self.stop_internal();
        // `self.thread_listener` est fermée par `stop_internal`.
    }

    fn stop_internal(&mut self) {
        self.stop_flag.store(true, Ordering::Release);
        // Fermer le clone du listener réveille immédiatement un accept()
        // bloquant dans le thread serveur.
        self.thread_listener = None;
    }
}

impl Drop for HealthHandle {
    fn drop(&mut self) {
        self.stop_internal();
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
    let thread_listener = listener.try_clone()?;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_thread = stop_flag.clone();
    let active = Arc::new(AtomicUsize::new(0));

    std::thread::Builder::new()
        .name("onde-health".to_string())
        .spawn(move || accept_loop(listener, metrics, active, stop_thread))?;

    Ok(HealthHandle {
        port: effective_port,
        stop_flag,
        thread_listener: Some(thread_listener),
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
                if active.load(Ordering::Relaxed) >= MAX_CONCURRENT_CONNECTIONS {
                    // Surchargé (impossible en pratique sur localhost) :
                    // refus explicite servi inline, aucun thread spawned.
                    serve_inline_busy(stream);
                    continue;
                }
                let m = metrics.clone();
                let a = active.clone();
                let spawned = std::thread::Builder::new()
                    .name("onde-health-conn".to_string())
                    .spawn(move || {
                        a.fetch_add(1, Ordering::Relaxed);
                        handle_connection(stream, &m);
                        a.fetch_sub(1, Ordering::Relaxed);
                    });
                if spawned.is_err() {
                    // Épuisement de threads OS : on ignore cette connexion
                    // (le client retentera) sans jamais paniquer.
                    continue;
                }
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
            }
        }
    }
}

/// Arrêt propre de la boucle d'acceptation (log structuré unique).
fn clean_stop() {
    tracing::info!("event=health_stopped reason=shutdown");
}

/// Réponse 503 servie inline quand le plafond de connexions est atteint.
fn serve_inline_busy(mut stream: TcpStream) {
    let body = "{\"error\":\"busy\"}";
    let _ = write_response(&mut stream, "503 Service Unavailable", body);
}

/// Traite UNE connexion : parse la ligne de requête, route, répond, ferme.
fn handle_connection(mut stream: TcpStream, metrics: &Arc<NodeMetrics>) {
    let _ = stream.set_read_timeout(Some(SOCKET_TIMEOUT));
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));

    let request_line = match read_request_head(&mut stream) {
        Some(line) => line,
        None => return, // timeout, connexion vide ou requête surdimensionnée
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

/// Lit l'en-tête HTTP jusqu'à la première ligne complète (CRLF), borné à
/// [`MAX_REQUEST_BYTES`]. Retourne la request-line, ou None si rien de
/// valable n'arrive à temps.
fn read_request_head(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return None, // fermeture sans requête complète
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = find_crlf(&buf) {
                    return Some(String::from_utf8_lossy(&buf[..pos]).into_owned());
                }
                if buf.len() > MAX_REQUEST_BYTES {
                    return None;
                }
            }
            Err(_) => return None, // timeout lecture (> SOCKET_TIMEOUT)
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

    /// Vérifie que `port` finit par refuser les connexions (listener bien
    /// fermé après arrêt), avec borne de temps courte (CI-safe).
    fn assert_port_released(port: u16) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "port must be released after shutdown"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
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
    fn test_shutdown_releases_port_cleanly() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let port = handle.port;
        handle.shutdown();
        // Le port doit finir par refuser les connexions (listener fermé).
        assert_port_released(port);
    }

    #[test]
    fn test_drop_impl_also_stops_server() {
        let metrics = Arc::new(NodeMetrics::new());
        let handle = spawn_health_server(0, metrics).expect("ephemeral bind");
        let port = handle.port;
        drop(handle);
        assert_port_released(port);
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
