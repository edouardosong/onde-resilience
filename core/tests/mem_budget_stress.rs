//! Phase 2.6 — Compression + budget mémoire : stress test gros volume.
//!
//! Critère d'acceptation ROADMAP : « Pas d'OOM à 100k messages ».
//!
//! Ce test ingère N alertes par le **chemin réel** d'un nœud receveur :
//! événement signé (Ed25519) → octets wire padés (`to_wire_bytes` /
//! `from_wire_bytes`) → [`Node::receive_peer_event`] (gate d'admission
//! anti-abus : signature, dédup, fenêtre glissante par auteur) →
//! [`Node::handle_incoming_alert`] (re-vérification signature + PoW +
//! réputation, insertion gossip bornée, stockage hiérarchique **compressé**
//! Deflate, persistance SQLite WAL).
//!
//! Deux profils d'auteurs, tous deux réalistes et couverts par le chemin réel :
//! - **Auteurs de confiance** (bulk) : voisins connus du mesh, score WoT ≥
//!   [`GENESIS_TRUST`] via `reputation.set_trusted` — même pattern que les
//!   tests existants (`node::tests`). Le PoW adaptatif leur impose une
//!   difficulté 0 : c'est le régime nominal d'un maillage établi, et il
//!   permet de pousser le volume complet sans que le coût CPU du PoW masque
//!   la mesure mémoire.
//! - **Auteurs inconnus** (sous-ensemble strict) : score WoT 0 → difficulté
//!   PoW maximale ([`MAX_POW_DIFFICULTY`]) exigée, nonce calculé par
//!   `compute_pow`. C'est le chemin le plus coûteux et le plus défensif —
//!   il est exercé en continu sur un sous-ensemble borné du volume.
//!
//! **CI-safe** (même politique que `ONDE_ZIM_FIXTURE` / `ONDE_MBTILES_FIXTURE`) :
//! - par défaut : N = 500 messages (quelques centaines — passe vite) ;
//! - `ONDE_STRESS=1` : N = 100_000 messages (validation gros volume complète).
//!
//! Mesure du pic RSS : `VmHWM` de `/proc/self/status` (hauteur d'eau haute du
//! processus, Linux — CI ubuntu-latest). Sur une autre plateforme la mesure
//! est indisponible : le test passe sur les bornes structurelles et signale.

use onde_core::crypto::Identity;
use onde_core::node::{Node, NodeConfig, NodeType, PeerEventOutcome};
use onde_core::protocol::{MeshEvent, OndeMessageType, MAX_KNOWN_EVENTS, MAX_PENDING_BROADCASTS};
use onde_core::reputation::{GENESIS_TRUST, MAX_POW_DIFFICULTY};
use onde_core::storage::{MessageTier, StoragePolicy, TieredMessageStore};

/// Volume par défaut (CI) et volume complet (`ONDE_STRESS=1`).
const VOLUME_CI: usize = 500;
const VOLUME_FULL: usize = 100_000;
/// Sous-ensemble « auteurs inconnus » (PoW maximal, chemin le plus strict).
const STRICT_CI: usize = 16;
const STRICT_FULL: usize = 128;
/// Auteurs de confiance tournés en rond pour le bulk.
const TRUSTED_AUTHORS: usize = 8;
/// Auteurs inconnus dédiés au sous-ensemble strict.
const UNKNOWN_AUTHORS: usize = 4;
/// Pas de temps injecté entre deux messages (s) : avec 8 auteurs en rotation
/// chacun publie toutes les 40 s (bulk) et avec 4 auteurs toutes les 20 s
/// (strict) — toujours bien sous le budget anti-spam (12/60 s par auteur).
const TIME_STEP_SECS: u64 = 5;

/// Seuil de pic RSS affirmé par ce test (MiB).
///
/// Justification : le profil Mobile borne le magasin hiérarchique à
/// `StoragePolicy::Mobile::max_bytes()` = 64 MiB **bruts** (les payloads
/// stockés sont compressés, donc ≤ bruts) ; les structures gossip sont
/// bornées (`MAX_KNOWN_EVENTS` IDs + outbox ≤ [`MAX_PENDING_BROADCASTS`]
/// événements), le garde-fou anti-spam est borné par auteur et le cache
/// SQLite WAL reste de l'ordre du Mo. Le pic mesuré à 100k messages (voir la
/// sortie du test) laisse une marge ×2 au-dessus : toute croissance non bornée
/// d'~1 Ko/message (+100 Mo à 100k) le dépasserait.
const RSS_PEAK_LIMIT_MIB: u64 = 256;

fn stress_volume() -> (usize, usize) {
    match std::env::var("ONDE_STRESS") {
        Ok(v) if v == "1" => (VOLUME_FULL, STRICT_FULL),
        _ => (VOLUME_CI, STRICT_CI),
    }
}

/// Pic RSS du processus en MiB (Linux : `VmHWM` de `/proc/self/status`).
fn peak_rss_mib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            // Format : "VmHWM:\t 12345 kB"
            let kb: u64 = rest.trim().trim_end_matches("kB").trim().parse().ok()?;
            return Some(kb.div_ceil(1024));
        }
    }
    None
}

/// Contenu d'alerte réaliste (~230 caractères, unique par index — sous la
/// limite `MAX_ALERT_SIZE` = 280 caractères).
fn alert_content(i: usize) -> String {
    format!(
        "ALERTE n°{i} : coupure électrique secteur Nord — générateur de secours disponible \
         au point de rassemblement place de la Mairie. Eau potable non concernée. \
         Prochain point d'information dans 30 minutes, canal radio 145.500 MHz."
    )
}

/// Construire un événement signé valide pour un **auteur de confiance** :
/// PoW adaptatif = difficulté 0 (aucun nonce à chercher — régime nominal).
fn make_trusted_alert(author: &Identity, content: String) -> MeshEvent {
    MeshEvent::new_signed(author, OndeMessageType::Alert, content, Vec::new())
        .with_pow_difficulty(0)
}

/// Construire un événement signé + PoW valides pour un **auteur inconnu** :
/// difficulté maximale ([`MAX_POW_DIFFICULTY`]) exigée par la réputation,
/// nonce calculé — le chemin d'admission le plus strict.
fn make_unknown_alert(author: &Identity, content: String) -> MeshEvent {
    let mut event = MeshEvent::new_signed(author, OndeMessageType::Alert, content, Vec::new());
    assert_eq!(event.pow_difficulty, MAX_POW_DIFFICULTY);
    assert!(
        event.compute_pow(2_000_000),
        "PoW doit réussir en < 2M itérations"
    );
    event
}

#[test]
fn stress_ingest_memory_bounded() {
    let (n, n_strict) = stress_volume();
    assert!(n >= n_strict);
    let n_bulk = n - n_strict;
    let dir = tempfile::tempdir().expect("tempdir pour SQLite");
    let db_path = dir
        .path()
        .join("onde-stress.db")
        .to_string_lossy()
        .into_owned();

    // Identité du receveur fixée (reproductible, restaurable par un 2e nœud).
    let receiver_seed = [9u8; 32];

    // Nœud receveur : profil **Mobile** (budget le plus serré, 64 MiB bruts)
    // + persistance SQLite réelle (WAL) — le chemin complet d'ingestion.
    let mut receiver = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        sqlite_path: Some(db_path.clone()),
        identity_seed: Some(receiver_seed),
        ..NodeConfig::default()
    });

    // Auteurs de confiance (bulk) — seeds déterministes, score WoT fondateur.
    let trusted: Vec<Identity> = (0..TRUSTED_AUTHORS)
        .map(|i| {
            let mut seed = [0u8; 32];
            seed[0] = i as u8 + 1;
            Identity::from_bytes(&seed)
        })
        .collect();
    for a in &trusted {
        receiver
            .reputation
            .set_trusted(&a.pubkey_hex(), GENESIS_TRUST);
    }

    // Auteurs inconnus (sous-ensemble strict) — aucune réputation initiale.
    let unknown: Vec<Identity> = (0..UNKNOWN_AUTHORS)
        .map(|i| {
            let mut seed = [0u8; 32];
            seed[0] = 100 + i as u8;
            Identity::from_bytes(&seed)
        })
        .collect();

    let baseline_rss = peak_rss_mib();
    let t0 = std::time::Instant::now();

    let mut stored = 0usize;
    let mut rejected = 0usize;
    let mut now: u64 = 1_700_000_000; // temps injecté, monotone croissant

    // --- Phase bulk : auteurs de confiance (régime nominal du mesh) --------
    for i in 0..n_bulk {
        let author = &trusted[i % TRUSTED_AUTHORS];
        let event = make_trusted_alert(author, alert_content(i));

        // Chemin réel : octets wire padés → décodage → gate + handler.
        let wire = event.to_wire_bytes().expect("encodage wire");
        let decoded = MeshEvent::from_wire_bytes(&wire).expect("décodage wire");
        match receiver.receive_peer_event(now, &decoded) {
            PeerEventOutcome::AlertStored => stored += 1,
            other => {
                rejected += 1;
                if rejected <= 3 {
                    eprintln!("bulk message {i}: {other:?}");
                }
            }
        }
        now = now.saturating_add(TIME_STEP_SECS);
    }

    // --- Phase stricte : auteurs inconnus, PoW maximal (chemin défensif) ---
    for i in 0..n_strict {
        let author = &unknown[i % UNKNOWN_AUTHORS];
        let event = make_unknown_alert(author, alert_content(n_bulk + i));

        let wire = event.to_wire_bytes().expect("encodage wire");
        let decoded = MeshEvent::from_wire_bytes(&wire).expect("décodage wire");
        match receiver.receive_peer_event(now, &decoded) {
            PeerEventOutcome::AlertStored => stored += 1,
            other => {
                rejected += 1;
                if rejected <= 3 {
                    eprintln!("strict message {i}: {other:?}");
                }
            }
        }
        now = now.saturating_add(TIME_STEP_SECS);
    }

    let elapsed = t0.elapsed();
    let peak_rss = peak_rss_mib();

    // --- Bornes structurelles (tiennent à 500 ET 100k) ---------------------
    assert_eq!(
        stored, n,
        "toutes les alertes doivent être stockées (budget Mobile 64 MiB >> {n} × ~250 B)"
    );
    assert_eq!(rejected, 0, "aucun rejet inattendu au volume {n}");
    assert!(
        receiver.gossip.known_count() <= MAX_KNOWN_EVENTS,
        "known_events {} > borne {}",
        receiver.gossip.known_count(),
        MAX_KNOWN_EVENTS
    );
    assert!(
        receiver.gossip.get_pending_broadcasts().len() <= MAX_PENDING_BROADCASTS,
        "outbox gossip non bornée"
    );
    let max_authors = TRUSTED_AUTHORS + UNKNOWN_AUTHORS;
    assert!(
        receiver.spam_guard.tracked_authors() <= max_authors,
        "le garde-fou ne doit suivre que les {max_authors} auteurs du corpus"
    );

    // --- Pic RSS borné -------------------------------------------------------
    println!(
        "stress: n={n} (bulk={n_bulk}, strict={n_strict}) stored={stored} rejected={rejected} \
         elapsed={}s baseline_rss={:?} MiB peak_rss={:?} MiB (seuil {} MiB)",
        elapsed.as_secs(),
        baseline_rss,
        peak_rss,
        RSS_PEAK_LIMIT_MIB
    );
    if let Some(peak) = peak_rss {
        assert!(
            peak <= RSS_PEAK_LIMIT_MIB,
            "pic RSS {peak} MiB > seuil {RSS_PEAK_LIMIT_MIB} MiB — croissance mémoire non bornée ?"
        );
    } else {
        println!("stress: VmHWM indisponible (non-Linux) — assertion de pic RSS sautée");
    }

    // --- Compression effective sur le corpus ingéré --------------------------
    let (raw, compressed) = receiver.message_store.compression_stats();
    assert!(
        raw > 0 && compressed < raw,
        "la compression doit réduire le stockage"
    );
    println!(
        "compression: brut={raw} B stocké={compressed} B ratio={:.2}",
        compressed as f64 / raw.max(1) as f64
    );

    // --- Persistance réelle : restauration par un 2e nœud (« redémarrage ») --
    let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    println!("sqlite: fichier={db_size} B (WAL inclus)");
    assert!(
        db_size > 0,
        "la persistance SQLite doit avoir reçu les messages"
    );

    let mut restarted = Node::new(NodeConfig {
        node_type: NodeType::Mobile,
        sqlite_path: Some(db_path),
        identity_seed: Some(receiver_seed),
        ..NodeConfig::default()
    });
    let restored = restarted
        .restore_from_persistence()
        .expect("restauration ok");
    assert_eq!(
        restored, n,
        "tous les messages persistés doivent être restaurables"
    );

    // Le pic après restauration reste borné (chemin de redémarrage inclus —
    // le 2e nœud tient sa propre copie du magasin en mémoire).
    if let Some(peak) = peak_rss_mib() {
        println!(
            "stress: post-restauration restored={restored} peak_rss={peak} MiB (seuil {} MiB)",
            RSS_PEAK_LIMIT_MIB
        );
        assert!(
            peak <= RSS_PEAK_LIMIT_MIB,
            "pic RSS post-restauration {peak} MiB > seuil {RSS_PEAK_LIMIT_MIB} MiB"
        );
    }
}

/// La compression Deflate du magasin hiérarchique réduit bien le stockage sur
/// un corpus compressible (textes répétés — cas typique des alertes de zone).
#[test]
fn compression_reduces_corpus_storage() {
    let mut store = TieredMessageStore::new(StoragePolicy::Mobile);
    // 2000 messages de texte hautement compressible (~1 Ko brut chacun).
    const N: usize = 2_000;
    const PAYLOAD: &str = "ALERTE : coupure électrique secteur Nord — générateur de secours \
        disponible au point de rassemblement. Eau potable non concernée. ";
    for i in 0..N {
        let mut payload = Vec::with_capacity(PAYLOAD.len() * 8);
        for _ in 0..8 {
            payload.extend_from_slice(PAYLOAD.as_bytes());
        }
        let ok = store
            .store(
                &format!("msg-{i}"),
                MessageTier::Critical,
                &payload,
                1_700_000_000,
                "u09tunq",
            )
            .expect("store ok");
        assert!(ok);
    }
    let (raw, compressed) = store.compression_stats();
    println!(
        "compression corpus compressible: brut={raw} B stocké={compressed} B ratio={:.2}",
        compressed as f64 / raw.max(1) as f64
    );
    assert!(
        compressed < raw / 2,
        "un texte répété doit se comprimer de plus de moitié (brut {raw}, stocké {compressed})"
    );
}
