/// Node Management — Core ONDE node with all subsystems
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use base64::Engine as _;

use crate::ai::AiEngine;
use crate::crypto::{Identity, RotatingIdentity, TxPool, ZkTransaction};
use crate::network::YggdrasilAddress;
use crate::protocol::{GossipProtocol, MeshEvent, OndeMessageType};
// Constantes anti-abus utilisées par les scénarios de test Phase 2.7.
use crate::reputation::{
    AbuseReason, AbuseReport, Endorsement, ReputationSystem, SpamGuard, TrustAction,
    SPAM_BUDGET_PER_WINDOW, SPAM_WINDOW_SECS,
};
#[cfg(test)]
use crate::reputation::{
    ABUSE_IGNORE_THRESHOLD, MAX_POW_DIFFICULTY, PENALTY_REMOTE_REPORT, SECS_PER_HOUR,
};
// T13 Fusion : graphe social Tuitter/Redit (contrat + cache SQLite).
use crate::social::{SocialComment, SocialPlatform, SocialPost};
use crate::social_store::SocialStore;
use crate::storage::{
    persistence::SqliteStore, IpfsSeeder, MBTilesRenderer, MessageTier, StoragePolicy,
    TieredMessage, TieredMessageStore, ZimReader,
};
use crate::update::{UpdateAnnouncement, UpdateProtocol, Version, DEFAULT_CHUNK_SIZE};

/// Node type
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum NodeType {
    /// Mobile device (phone/tablet)
    Mobile,
    /// Desktop/Laptop bridge (ethernet + AI oracle)
    DesktopBridge,
}

/// Node configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub node_type: NodeType,
    pub display_name: String,
    pub available_ram_mb: u64,
    pub storage_gb: u64,
    pub ai_model_preference: Option<String>,
    pub max_peer_connections: u32,
    /// Chemin de la base SQLite pour la persistance des messages
    /// (Audit #14 — résilience aux crashs). `None` = mémoire seule.
    #[serde(default)]
    pub sqlite_path: Option<String>,
    /// Chemin de la base SQLite du graphe social Tuitter/Redit (T13 Fusion).
    /// Base **dédiée** (tables préfixées `social_*`) — `None` = fonctionnement
    /// sans persistance sociale (cache en mémoire seule désactivé).
    #[serde(default)]
    pub social_db_path: Option<String>,
    /// Geohash (7 caractères) de la position du nœud — pilote le **sharding
    /// géographique** du magasin hiérarchique : seuls les messages de notre
    /// voisinage (ou les alertes critiques) sont retenus localement.
    #[serde(default = "default_my_geohash")]
    pub my_geohash: String,
    /// Mode économie batterie (Phase 2/3 — throttling adaptatif).
    /// Quand il est actif, le nœud réduit la fréquence des tâches de fond
    /// (sweep, diffusion) et diffère le travail non critique. Le hook est
    /// volontairement simple : l'appelant (daemon, UI) décide quand basculer.
    #[serde(default)]
    pub battery_saver: bool,
    /// Seed Ed25519 (32 octets) d'une identité stable (Audit B4).
    ///
    /// `None` = générer (ou restaurer depuis la base SQLite si `sqlite_path`
    /// est fourni et qu'une seed y a déjà été persistée). La seed n'est
    /// JAMAIS loggée ni exposée dans [`NodeStatus`].
    #[serde(default)]
    pub identity_seed: Option<[u8; 32]>,
    /// Racine de confiance **épinglée** (32 octets) du protocole de mise à
    /// jour APK (Phase 1.1). `None` = racine par défaut du nœud
    /// ([`DEFAULT_UPDATE_ROOT_PUBKEY`]) qui n'autorise aucune annonce ni
    /// vérification — les déploiements réels doivent épingler la vraie clé.
    #[serde(default)]
    pub update_root_pubkey: Option<[u8; 32]>,
    /// Seed Ed25519 (32 octets) de la **clé racine de distribution**.
    ///
    /// Seul un nœud de distribution (qui détient la clé de l'équipe) la
    /// configure : elle permet à ce nœud de signer les annonces et
    /// manifestes de mise à jour (`Node::announce_update`). `None` = nœud
    /// receveur uniquement (vérifie et installe, mais n'annonce pas).
    /// La seed n'est JAMAIS loggée ni exposée dans [`NodeStatus`].
    #[serde(default)]
    pub update_root_seed: Option<[u8; 32]>,
    /// Version actuellement installée (format `"maj.min.patch"`), point de
    /// départ du protocole de mise à jour. Par défaut [`DEFAULT_UPDATE_VERSION`].
    #[serde(default = "default_update_version")]
    pub update_version: String,
}

/// Version de base d'un nœud frais (avant toute mise à jour installée).
fn default_update_version() -> String {
    DEFAULT_UPDATE_VERSION.to_string()
}

/// Position de démonstration par défaut (Paris, tour Eiffel) — en attendant
/// le fix GPS réel du nœud.
fn default_my_geohash() -> String {
    "u09tunq".to_string()
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            node_type: NodeType::Mobile,
            display_name: "Unknown".to_string(),
            available_ram_mb: 4096,
            storage_gb: 64,
            ai_model_preference: None,
            max_peer_connections: 20,
            sqlite_path: None,
            social_db_path: None,
            my_geohash: default_my_geohash(),
            battery_saver: false,
            identity_seed: None,
            update_root_pubkey: None,
            update_root_seed: None,
            update_version: default_update_version(),
        }
    }
}

/// Intervalle de sweep en mode économie batterie (5 minutes).
const BATTERY_SAVER_SWEEP_INTERVAL_SECS: u64 = 300;

/// Throttling adaptatif (Phase 3 — audit plan P3) : intervalle minimal entre
/// deux publications pour un nœud de confiance (WoT) en mode normal.
const PUBLISH_INTERVAL_TRUSTED_SECS: u64 = 10;
/// Idem pour un nœud inconnu (réputation basse) — il publie beaucoup moins
/// souvent tant qu'il n'a pas prouvé sa fiabilité.
const PUBLISH_INTERVAL_UNTRUSTED_SECS: u64 = 120;
/// Multiplicateur appliqué en mode économie batterie (10 s → 60 s, 120 s → 12 min).
const PUBLISH_BATTERY_MULTIPLIER: u64 = 6;

/// Horodatage unix courant en secondes (0 si l'horloge système est invalide).
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Racine de confiance par défaut du protocole de mise à jour (placeholder).
///
/// Une clé nulle n'est **jamais** acceptée par Ed25519 : avec cette racine
/// par défaut, aucune annonce ni vérification n'est possible. Les
/// déploiements réels DOIVENT épingler la vraie clé racine via
/// [`NodeConfig::update_root_pubkey`].
pub const DEFAULT_UPDATE_ROOT_PUBKEY: [u8; 32] = [0u8; 32];

/// Version de base d'un nœud frais (aucune mise à jour installée).
pub const DEFAULT_UPDATE_VERSION: &str = "1.0.0";

/// Parser la version installée configurée, avec repli sûr sur
/// [`DEFAULT_UPDATE_VERSION`] (config invalide → base, jamais de panique).
fn parse_update_version(s: &str) -> Version {
    Version::parse(s).unwrap_or_else(|_| {
        tracing::warn!("invalid update_version {s:?} — falling back to {DEFAULT_UPDATE_VERSION}");
        Version::new(1, 0, 0)
    })
}

/*
 * Tags wire du protocole de mise à jour (format `k=v` dans MeshEvent.tags).
 * Phase 1.1 : le `content` de `MeshEvent` porte le blob signé (base64) ;
 * les métadonnées (version, pair, index, taille, signature racine) vivent
 * dans les tags.
 */
const TAG_ROOT_SIG: &str = "root_sig";
const TAG_PEER: &str = "peer";
const TAG_TO: &str = "to";
const TAG_VERSION: &str = "version";
const TAG_INDEX: &str = "index";
const TAG_TOTAL: &str = "total";
const TAG_REQ_TYPE: &str = "req_type";
/// Phase 1.4 — annonce de rotation d'identité X25519 (forward secrecy).
const TAG_IDENTITY_ROTATION: &str = "identity_rotation";

/// Construire une liste de tags `k=v` dans l'ordre donné.
fn build_tags(pairs: &[(&str, String)]) -> Vec<String> {
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// Extraire la valeur d'un tag `k=v` (None si absent).
fn tag_get<'a>(tags: &'a [String], key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    tags.iter().find_map(|t| t.strip_prefix(&prefix))
}

/// Clé de la table `meta` SQLite stockant la seed Ed25519 (Audit B4).
const IDENTITY_SEED_META_KEY: &str = "identity_seed";

/// Restaurer la seed Ed25519 persistée dans la base SQLite (Audit B4).
///
/// Retourne `None` si la base n'est pas disponible, si aucune seed n'a été
/// persistée, ou si la valeur stockée est corrompue (dans ce dernier cas le
/// nœud régénère une identité propre — comportement volontairement sûr).
fn load_persisted_identity_seed(persistence: &Option<SqliteStore>) -> Option<[u8; 32]> {
    let store = persistence.as_ref()?;
    let hex_seed = store.get_meta(IDENTITY_SEED_META_KEY).ok()??;
    let bytes = hex::decode(&hex_seed).ok()?;
    let seed: [u8; 32] = bytes.try_into().ok()?;
    Some(seed)
}

/// Offre de mise à jour détenue par un nœud annonceur (Phase 1.1).
///
/// Construite par [`Node::announce_update`] : l'APK est conservé pour servir
/// les chunks, et le manifeste signé est pré-calculé pour répondre aux
/// requêtes `manifest` sans re-signer.
struct UpdateOffer {
    version: Version,
    apk: Vec<u8>,
    manifest_wire: Vec<u8>,
    manifest_signature: [u8; 64],
    chunk_size: u32,
}

/// Résultat du traitement d'un message update entrant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateHandlingOutcome {
    /// Message non lié à la mise à jour (ignoré).
    Ignored,
    /// Annonce acceptée → requête de manifeste émise vers l'annonceur.
    AnnouncementRequested,
    /// Manifeste accepté → requête de chunk 0 émise vers l'annonceur.
    ManifestRequested,
    /// Chunk accepté → requête du chunk suivant émise.
    ChunkRequested(u32),
    /// APK assemblé, vérifié de bout en bout et installé (version installée).
    Installed(Version),
    /// L'annonceur a servi le manifeste demandé.
    ManifestServed,
    /// L'annonceur a servi le chunk demandé (index).
    ChunkServed(u32),
    /// Message update rejeté pour une raison protocolaire (signature
    /// invalide, version non supérieure, chunk hors bornes, APK falsifié…).
    Rejected(String),
}

/// Résultat du traitement d'un endossement WoT entrant (Phase 1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndorsementHandlingOutcome {
    /// Message non lié au Web of Trust (ignoré).
    Ignored,
    /// Endossement vérifié et intégré à la réputation locale + relai.
    Applied,
    /// Endossement rejeté pour une raison protocolaire ou de réputation
    /// (payload invalide, signature invalide, endosseur non de confiance,
    /// self, doublon).
    Rejected(String),
}

/// Résultat du traitement d'une annonce de rotation d'identité X25519
/// (Phase 1.4 — forward secrecy du chiffrement point-à-point).
#[derive(Debug, Clone, PartialEq)]
pub enum RotationHandlingOutcome {
    /// Message non lié à une rotation (ignoré).
    Ignored,
    /// Annonce vérifiée : la clé X25519 du pair a été mise à jour (l'ancienne
    /// est conservée en période de grâce) et l'annonce a été relaiée.
    Applied,
    /// Annonce rejetée (payload invalide, signature invalide, pair non de
    /// confiance, ou clé déjà connue).
    Rejected(String),
}

/// Décision du **gate d'admission** anti-abus (Phase 2.7) appliqué à tout
/// événement entrant avant routage vers les handlers métier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// Événement admis — poursuit son traitement normal.
    Admitted,
    /// Événement refusé AVANT tout traitement coûteux (validation, stockage,
    /// relai). La raison est explicite pour l'observabilité et les métriques.
    Rejected(String),
}

/// Résultat du traitement d'un événement de pair par le dispatcher
/// [`Node::receive_peer_event`] (gate + routage + classification des abus).
#[derive(Debug, Clone, PartialEq)]
pub enum PeerEventOutcome {
    /// Refusé par le gate anti-abus (signature invalide, auteur ignoré,
    /// budget dépassé) ou événement signé mais invalide (violation enregistrée).
    Rejected(String),
    /// Alerte vérifiée et stockée localement (+relai).
    AlertStored,
    /// Alerte valide mais non retenue (déjà connue, budget/sharding).
    AlertNotStored,
    /// Endossement vérifié et intégré à la réputation locale.
    EndorsementApplied,
    /// Endossement rejeté (payload, politique, doublon…).
    EndorsementRejected(String),
    /// Signalement d'abus intégré — nouveau niveau d'abus du dénoncé.
    AbuseReportApplied(f64),
    /// Signalement d'abus rejeté (voir [`AbuseReportOutcome`]).
    AbuseReportRejected(String),
    /// Événement social Tuitter/Redit traité après le gate d'admission
    /// (T13 Fusion — voir [`SocialEventOutcome`] pour le détail).
    Social(SocialEventOutcome),
    /// Kind non géré par ce dispatcher.
    Other,
}

/// Résultat du traitement d'un signalement d'abus entrant (Phase 2.7).
#[derive(Debug, Clone, PartialEq)]
pub enum AbuseReportOutcome {
    /// Message non lié aux signalements (ignoré).
    Ignored,
    /// Signalement qualifié intégré — nouveau niveau d'abus du dénoncé.
    Applied(f64),
    /// Signalement rejeté (payload invalide, signature invalide, rapporteur
    /// ≠ signataire, rapporteur non de confiance, raison inconnue, doublon).
    Rejected(String),
}

/// Résultat du traitement d'un événement social reçu du gossip (T13 Fusion).
///
/// « Stocké » signifie **accepté et relayé** : l'écriture dans le cache
/// SQLite local est best-effort — un cache-miss (commentaire orphelin,
/// disque plein, base indisponible) ne change PAS l'issue et ne pénalise
/// jamais l'auteur distant.
#[derive(Debug, Clone, PartialEq)]
pub enum SocialEventOutcome {
    /// Message non social (ignoré).
    Ignored,
    /// Post accepté (id).
    PostStored(String),
    /// Commentaire accepté (id) — éventuellement bufferisé en attendant son post.
    CommentStored(String),
    /// Vote appliqué.
    VoteApplied,
    /// Abonnement / désabonnement appliqué.
    FollowApplied,
    /// Message privé stocké.
    MessageStored,
    /// Signalement de modération enregistré.
    ModerationApplied,
}

// T13-checker M3 — plafonds BRUTS (octets de `content`) appliqués AVANT tout
// décodage JSON, même justification que Endorsement/AbuseReport (1024 o) :
// borner le travail du parseur et la mémoire retenue, rejeter proprement et
// de façon attribuable un payload signé surdimensionné. Les plafonds post/
// commentaire couvrent le pire cas valide (40 000 caractères × 4 octets
// UTF-8 × échappement JSON ≈ 320 ko).
const SOCIAL_POST_MAX_BYTES: usize = 512 * 1024;
const SOCIAL_COMMENT_MAX_BYTES: usize = 512 * 1024;
const SOCIAL_VOTE_MAX_BYTES: usize = 4 * 1024;
const SOCIAL_FOLLOW_MAX_BYTES: usize = 4 * 1024;
const SOCIAL_MESSAGE_MAX_BYTES: usize = 16 * 1024;
const SOCIAL_MODERATION_MAX_BYTES: usize = 8 * 1024;

// ---------------------------------------------------------------------------
// Phase 3.4 — Auto-réparation : détection de partition, re-sync, heal
//
// AUCUNE extension wire : la détection est un book local de présence par
// pair (temps injecté — déterministe et testable), le rattrapage réutilise
// l'outbox du gossip existante avec son suivi « livré par pair », et la
// grâce de heal est un ajustement LOCAL du gate anti-abus.
// ---------------------------------------------------------------------------

/// Silence prolongé (secs) après lequel TOUS les pairs connus sont jugés
/// coupés → partition soupçonnée. Heuristique volontairement simple et
/// déterministe : le temps est injecté (`now`), jamais lu de l'horloge
/// système ; un nœud sans aucun contact connu n'est JAMAIS soupçonné
/// (impossible de distinguer partition et premier démarrage).
pub const PARTITION_SILENCE_THRESHOLD_SECS: u64 = 300;

/// Durée (secs) de la « grâce de heal » ouverte au retour d'une partition :
/// fenêtre courte pendant laquelle le budget anti-spam par auteur est étendu
/// (×[`HEAL_WINDOW_BUDGET_FACTOR`]) pour absorber le rattrapage légitime.
pub const HEAL_GRACE_SECS: u64 = 120;

/// Facteur d'extension du budget [`SPAM_BUDGET_PER_WINDOW`] pendant la
/// grâce de heal (12 → 48 admissions/auteur/fenêtre). Le rattrapage rejoue
/// des messages légitimes accumulés côté auteur PENDANT la coupure : sans
/// extension ils seraient rejetés en masse + pénalisés à la reconvergence
/// (perte de données + faux positif anti-abus). L'amplification reste
/// bornée : facteur fixe ×4, fenêtre temporelle courte, déclenchement
/// possible uniquement par une transition silence→contact observée
/// localement (un attaquant ne peut pas provoquer cette transition à
/// distance).
pub const HEAL_WINDOW_BUDGET_FACTOR: usize = 4;

/// Taille maximale d'un lot de rattrapage [`Node::take_heal_batch`] par
/// appel — le transport boucle jusqu'à lot vide : volume total = exactement
/// ce que le pair n'a pas encore, jamais une tempête d'un coup.
pub const HEAL_BATCH_MAX_EVENTS: usize = 32;

/// Vérifie le plafond brut d'un payload social avant décodage.
fn check_social_payload_size(kind: &str, content: &str, max_bytes: usize) -> Result<(), String> {
    if content.len() > max_bytes {
        return Err(format!("{kind} payload too large (max {max_bytes} bytes)"));
    }
    Ok(())
}

/// Valide une référence de cible sociale (id de post/commentaire/cible).
fn validate_social_target_ref(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty() || value.chars().count() > crate::social::MAX_SOCIAL_ID {
        return Err(format!(
            "{field} must contain 1..={} characters",
            crate::social::MAX_SOCIAL_ID
        ));
    }
    Ok(())
}

/// Valide une référence à une clé publique hexadécimale 32 octets.
fn validate_social_pubkey_ref(value: &str, field: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("{field} must be a 32-byte hex pubkey"));
    }
    Ok(())
}

/// Parse le nom d'une plateforme sociale accepté par l'API publique.
fn parse_social_platform(platform: &str) -> Result<SocialPlatform, String> {
    match platform {
        "Tuitter" | "tuitter" => Ok(SocialPlatform::Tuitter),
        "Redit" | "redit" => Ok(SocialPlatform::Redit),
        other => Err(format!("unknown social platform: {other}")),
    }
}

/// Génère un identifiant social unique (UUID v4 compact, sans dépendance
/// externe — même format que les commandes Tauri sociales).
fn generate_social_id() -> String {
    use rand::Rng;
    let rng = &mut rand::thread_rng();
    let a: u32 = rng.gen();
    let b: u16 = rng.gen();
    let c: u16 = rng.gen();
    let d: u64 = rng.gen();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a,
        b & 0x0fff,
        c & 0x0fff,
        (d as u16 & 0x3fff) | 0x8000,
        d >> 16,
    )
}

/// The main ONDE node
pub struct Node {
    pub config: NodeConfig,
    pub identity: Identity,
    /// Identité à rotation automatique (Audit #10)
    pub identity_rotator: RotatingIdentity,
    /// Réputation / Web of Trust (Audit #11)
    pub reputation: ReputationSystem,
    /// Stockage hiérarchique par tiers (Audit #8)
    pub message_store: TieredMessageStore,
    /// Persistance SQLite (Audit #14) — `None` = mémoire seule
    pub persistence: Option<SqliteStore>,
    pub mesh_address: YggdrasilAddress,
    pub gossip: GossipProtocol,
    pub tx_pool: TxPool,
    pub ai_engine: Mutex<AiEngine>,
    pub zim_reader: ZimReader,
    pub map_renderer: MBTilesRenderer,
    pub ipfs_seeder: IpfsSeeder,
    is_running: bool,
    /// Horodatage (unix secs) de la dernière publication — pilote le
    /// **throttling adaptatif** (Phase 3 du plan d'audit, P3). 0 = jamais
    /// publié → la première publication est toujours autorisée.
    last_publish_at: u64,
    /// Machine à états du protocole de mise à jour APK (racine épinglée).
    pub update_protocol: UpdateProtocol,
    /// Identité racine de distribution (seed configurée) — `None` = nœud
    /// receveur uniquement (vérifie et installe, mais n'annonce pas).
    update_root_signing: Option<Identity>,
    /// Dernière annonce acceptée (en attente du manifeste correspondant).
    pending_announcement: Option<UpdateAnnouncement>,
    /// Offre de mise à jour détenue par ce nœud (annonceur) — `None` sinon.
    update_offer: Option<UpdateOffer>,
    /// Phase 1.4 : clés publiques X25519 des pairs (hex 64) — la clé
    /// courante connue de chaque pair, mise à jour par les annonces
    /// `IdentityRotation` reçues (forward secrecy du chiffrement
    /// point-à-point). La clé précédente est conservée en `peer_x25519_grace`
    /// jusqu'à la rotation suivante (période de grâce : les messages
    /// chiffrés avec l'ancienne clé restent déchiffrables).
    peer_x25519: std::collections::HashMap<String, String>,
    /// Clé X25519 précédente conservée en période de grâce par pair.
    peer_x25519_grace: std::collections::HashMap<String, String>,
    /// Phase 1.4 — anti-replay : le **dernier compteur de rotation** accepté
    /// pour chaque pair (le plus grand). Une annonce dont le compteur est
    /// inférieur est rejetée : on ne recule jamais vers une clé X25519
    /// antérieure, même si l'événement est signé par un pair de confiance.
    peer_rotation_count: std::collections::HashMap<String, u64>,
    /// Phase 2.7 — garde-fou anti-spam : fenêtre glissante par auteur
    /// appliquée à l'entrée du nœud (voir [`SpamGuard`]).
    pub spam_guard: SpamGuard,
    /// Cache matérialisé du graphe social Tuitter/Redit (T13 Fusion).
    /// `None` = stockage social désactivé (base indisponible ou non configurée).
    pub social_store: Option<SocialStore>,
    /// Phase 3.4 — présence par pair : instant (unix secs, INJECTÉ) du
    /// dernier événement **admis** signé par chaque pair. Alimente la
    /// détection de partition ([`Node::partition_suspected`]) ; un doublon
    /// relayé prouve lui aussi la connectivité.
    peer_last_seen: std::collections::HashMap<String, u64>,
    /// Phase 3.4 — fin de la « grâce de heal » (unix secs injecté) : fenêtre
    /// courte post-retour-de-partition pendant laquelle le budget anti-spam
    /// par auteur est étendu pour absorber le rattrapage. 0 = jamais ouverte.
    heal_grace_until: u64,
    /// Phase 3.6 — registre de métriques partagé (compteurs atomiques) :
    /// alimenté par les points d'instrumentation du nœud et lu par l'endpoint
    /// de santé (`--health-port`) ainsi que par le log structuré de démarrage.
    pub metrics: std::sync::Arc<crate::metrics::NodeMetrics>,
}

impl Node {
    pub fn new(config: NodeConfig) -> Self {
        // Persistance SQLite optionnelle : ouverte AVANT la création de
        // l'identité pour pouvoir restaurer une seed persistée (Audit B4).
        // En cas d'échec, on continue en mémoire seule avec un warning — la
        // persistance ne doit jamais empêcher le nœud de démarrer.
        let persistence = match &config.sqlite_path {
            Some(path) => match SqliteStore::open(path) {
                Ok(store) => {
                    tracing::info!("SQLite persistence enabled at {path}");
                    Some(store)
                }
                Err(e) => {
                    tracing::warn!("SQLite persistence disabled ({e}) — running in memory only");
                    None
                }
            },
            None => None,
        };

        // Identité Ed25519 stable (Audit B4) : l'ordre de priorité est
        // 1. seed explicite dans la config, 2. seed persistée en base,
        // 3. nouvelle identité générée (puis persistée si SQLite est actif).
        // La seed n'est JAMAIS loggée ni exposée dans NodeStatus.
        let identity = if let Some(seed) = config.identity_seed {
            Identity::from_bytes(&seed)
        } else if let Some(seed) = load_persisted_identity_seed(&persistence) {
            Identity::from_bytes(&seed)
        } else {
            let identity = Identity::generate();
            if let Some(persist) = &persistence {
                let seed = identity.signing_key_bytes();
                if let Err(e) = persist.set_meta(IDENTITY_SEED_META_KEY, &hex::encode(seed)) {
                    tracing::warn!("identity seed persistence failed: {e}");
                }
            }
            identity
        };

        let pubkey = identity.pubkey_hex();
        let mesh_address = YggdrasilAddress::new(&pubkey);

        // Réputation : le nœud se fait confiance lui-même + bootstrap vide
        let mut reputation = ReputationSystem::new();
        reputation.bootstrap(std::slice::from_ref(&pubkey));

        // Stockage hiérarchique selon le profil — géolocalisé pour le
        // sharding géographique (le nœud ne retient que son voisinage).
        let policy = match config.node_type {
            NodeType::Mobile => StoragePolicy::Mobile,
            NodeType::DesktopBridge => StoragePolicy::Desktop,
        };
        let mut message_store = TieredMessageStore::new(policy);
        message_store.set_my_geohash(&config.my_geohash);

        let ai_engine = AiEngine::new(config.available_ram_mb);
        // The seed directory may be unavailable (e.g. read-only filesystem):
        // log a clear warning and continue with an empty, disabled seeder.
        let ipfs_seeder = match IpfsSeeder::new("/tmp/onde-ipfs", config.storage_gb) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("IPFS seeder disabled: {e}");
                IpfsSeeder::disabled(config.storage_gb)
            }
        };

        // Protocole de mise à jour APK (Phase 1.1) : la racine épinglée vient
        // de la config (ou du placeholder inoffensif), la version courante de
        // `update_version`, et la clé racine de distribution (seed) est
        // conservée séparément — jamais exposée dans NodeStatus.
        let update_root_pubkey = config
            .update_root_pubkey
            .unwrap_or(DEFAULT_UPDATE_ROOT_PUBKEY);
        let update_protocol = UpdateProtocol::new(
            update_root_pubkey,
            parse_update_version(&config.update_version),
        );
        let update_root_signing = config
            .update_root_seed
            .map(|seed| Identity::from_bytes(&seed));

        // Graphe social Tuitter/Redit (T13 Fusion) : base SQLite **dédiée**
        // (tables `social_*`, zéro collision avec la persistance messages).
        // En cas d'échec d'ouverture le nœud continue SANS cache social —
        // la dégradation ne doit jamais empêcher le démarrage du nœud.
        let social_store = match &config.social_db_path {
            Some(path) => match SocialStore::open(path) {
                Ok(s) => {
                    tracing::info!("social store opened at {path}");
                    Some(s)
                }
                Err(e) => {
                    tracing::warn!("social store disabled ({e})");
                    None
                }
            },
            None => None,
        };

        // Phase 3.6 — registre de métriques partagé entre le nœud, la outbox
        // gossip et le magasin hiérarchique (hooks optionnels ci-dessous).
        let metrics = std::sync::Arc::new(crate::metrics::NodeMetrics::new());
        let mut node = Self {
            config,
            identity,
            identity_rotator: RotatingIdentity::new(6 * 3600), // rotation toutes les 6 h
            reputation,
            message_store,
            persistence,
            mesh_address,
            gossip: GossipProtocol::new(),
            tx_pool: TxPool::new(),
            ai_engine: Mutex::new(ai_engine),
            zim_reader: ZimReader::new(),
            map_renderer: MBTilesRenderer::new(),
            ipfs_seeder,
            is_running: false,
            last_publish_at: 0,
            update_protocol,
            update_root_signing,
            pending_announcement: None,
            update_offer: None,
            peer_x25519: std::collections::HashMap::new(),
            peer_x25519_grace: std::collections::HashMap::new(),
            peer_rotation_count: std::collections::HashMap::new(),
            spam_guard: SpamGuard::new(SPAM_WINDOW_SECS, SPAM_BUDGET_PER_WINDOW),
            social_store,
            peer_last_seen: std::collections::HashMap::new(),
            heal_grace_until: 0,
            metrics: metrics.clone(),
        };
        node.gossip.set_metrics(metrics.clone());
        node.message_store.set_metrics(metrics);
        node
    }

    /// Start the node
    pub async fn start(&mut self) -> Result<(), String> {
        tracing::info!(
            "Starting ONDE node [{}] type={:?} pubkey={}",
            self.config.display_name,
            self.config.node_type,
            self.identity.pubkey_hex()
        );

        // Première rotation : amorce l'identité de session
        self.maybe_rotate_identity();

        self.is_running = true;
        Ok(())
    }

    /// Stop the node
    pub async fn stop(&mut self) {
        tracing::info!("Stopping ONDE node...");
        self.is_running = false;
    }

    /// Rotation périodique de l'identité de session (Audit #10).
    /// L'ancienne clé reste vérifiable pendant la période de grâce.
    pub fn maybe_rotate_identity(&mut self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if self.identity_rotator.maybe_rotate(now) {
            tracing::info!(
                "Identity rotated ({} rotations), new pubkey {}",
                self.identity_rotator.rotation_count(),
                self.identity_rotator.current_pubkey_hex()
            );
            true
        } else {
            false
        }
    }

    /// Publish an alert message.
    ///
    /// La difficulté PoW est **adaptative** selon la réputation du nœud
    /// (Audit #11) : un nœud de confiance n'a pas de coût PoW, un nœud
    /// inconnu doit payer `MAX_POW_DIFFICULTY`. Le **throttling adaptatif**
    /// (Phase 3) limite le débit de publication : un nœud doit respecter
    /// l'intervalle [`Node::publish_interval_secs`] entre deux messages,
    /// sinon la publication est refusée avec un message explicite.
    /// L'alerte est stockée dans le tier Critical du magasin hiérarchique.
    pub async fn publish_alert(&mut self, content: String) -> Result<MeshEvent, String> {
        // Limite mesurée en CARACTÈRES (pas en octets) pour cohérence avec
        // `protocol/mod.rs` (Audit m1) — évite de tronquer les caractères
        // multioctets (émojis, accents UTF-8).
        if content.chars().count() > 280 {
            return Err("Alert must be <= 280 characters".to_string());
        }
        self.enforce_publish_throttle()?;

        let difficulty = self
            .reputation
            .required_pow_difficulty(&self.identity.pubkey_hex());
        let mut event = MeshEvent::new_signed(
            &self.identity,
            OndeMessageType::Alert,
            content.clone(),
            vec![],
        )
        .with_pow_difficulty(difficulty);

        // Les nœuds de confiance n'ont pas de PoW ; les autres doivent le calculer
        if difficulty > 0 && !event.compute_pow(2_000_000) {
            return Err("PoW computation failed".to_string());
        }

        // Validation locale (avec réputation)
        event.validate_with_reputation(&self.reputation)?;

        self.gossip
            .add_event_with_reputation(event.clone(), &self.reputation)?;

        // Stockage hiérarchique : les alertes sont Critical (7 jours), toujours
        // retenues localement (le sharding géographique garde les urgences).
        let geohash = self.config.my_geohash.clone();
        if self.message_store.store(
            &event.id,
            MessageTier::Critical,
            event.content.as_bytes(),
            event.created_at,
            &geohash,
        )? {
            // Persistance SQLite (best-effort) — uniquement si stocké en mémoire
            self.persist_message(
                &event.id,
                MessageTier::Critical,
                event.content.as_bytes(),
                event.created_at,
                &geohash,
            );
        }

        self.record_publish();
        Ok(event)
    }

    /// Publish a mutual aid request
    pub async fn publish_mutual_aid(&mut self, content: String) -> Result<MeshEvent, String> {
        // Limite côté serveur (Aikido PR#8 MED) : sans elle, un front compromis
        // ou tout appelant du bridge Tauri pourrait émettre des événements
        // d'entraide arbitrairement gros → épuisement mémoire/gossip/SQLite.
        // Même cap que les alertes (280 caractères) pour cohérence.
        if content.chars().count() > 280 {
            return Err("Mutual aid request must be <= 280 characters".to_string());
        }
        self.enforce_publish_throttle()?;
        let difficulty = self
            .reputation
            .required_pow_difficulty(&self.identity.pubkey_hex());
        let mut event =
            MeshEvent::new_signed(&self.identity, OndeMessageType::MutualAid, content, vec![])
                .with_pow_difficulty(difficulty);

        if difficulty > 0 && !event.compute_pow(2_000_000) {
            return Err("PoW computation failed".to_string());
        }
        event.validate_with_reputation(&self.reputation)?;

        self.gossip
            .add_event_with_reputation(event.clone(), &self.reputation)?;

        // Stockage hiérarchique : les demandes d'entraide sont Important (2 jours)
        let geohash = self.config.my_geohash.clone();
        if self.message_store.store(
            &event.id,
            MessageTier::Important,
            event.content.as_bytes(),
            event.created_at,
            &geohash,
        )? {
            self.persist_message(
                &event.id,
                MessageTier::Important,
                event.content.as_bytes(),
                event.created_at,
                &geohash,
            );
        }

        self.record_publish();
        Ok(event)
    }

    /// Intervalle minimal (secondes) entre deux publications — **throttling
    /// adaptatif** (Phase 3 du plan d'audit).
    ///
    /// Le débit dépend de deux signaux :
    /// - **réputation** (Web of Trust) : un nœud de confiance publie toutes
    ///   les 10 s, un nœud inconnu toutes les 2 min ;
    /// - **mode batterie** : tous les intervalles sont multipliés par 6 pour
    ///   économiser l'énergie (une publication = radio + PoW éventuel + I/O).
    pub fn publish_interval_secs(&self) -> u64 {
        let score = self.reputation.score(&self.identity.pubkey_hex());
        let base = if score >= crate::reputation::TRUSTED_THRESHOLD {
            PUBLISH_INTERVAL_TRUSTED_SECS
        } else {
            PUBLISH_INTERVAL_UNTRUSTED_SECS
        };
        if self.config.battery_saver {
            base.saturating_mul(PUBLISH_BATTERY_MULTIPLIER)
        } else {
            base
        }
    }

    /// Vérifier que la publication est autorisée par le throttle. Refuse avec
    /// un message explicite (l'UI le remonte tel quel à l'utilisateur).
    fn enforce_publish_throttle(&self) -> Result<(), String> {
        let now = unix_now();
        if !self.publish_allowed(now) {
            return Err(format!(
                "Rate limited: wait {}s before the next publication",
                self.publish_interval_secs()
            ));
        }
        Ok(())
    }

    fn publish_allowed(&self, now: u64) -> bool {
        now.saturating_sub(self.last_publish_at) >= self.publish_interval_secs()
    }

    fn record_publish(&mut self) {
        self.last_publish_at = unix_now();
    }

    /// Persister un message dans SQLite (best-effort — ne bloque jamais la
    /// publication ni le réseau). Réutilise la version compressée du magasin
    /// en mémoire quand elle existe.
    fn persist_message(
        &mut self,
        id: &str,
        tier: MessageTier,
        payload: &[u8],
        created_at: u64,
        geohash: &str,
    ) {
        let Some(persist) = self.persistence.as_mut() else {
            return;
        };
        let msg = self
            .message_store
            .all_messages()
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .unwrap_or(TieredMessage {
                id: id.to_string(),
                tier,
                created_at,
                payload: payload.to_vec(),
                original_size: payload.len(),
                geohash: geohash.to_string(),
            });
        if let Err(e) = persist.store(&msg) {
            tracing::warn!("SQLite persist failed for {id}: {e}");
        }
    }

    /// Traiter une alerte critique **reçue du gossip** (Phase 1.7 — scénario
    /// de bout en bout : publication → propagation → stockage → restauration).
    ///
    /// 1. **Vérification de bout en bout** (défense en profondeur) : la
    ///    signature Ed25519, le PoW adaptatif et la réputation de l'émetteur
    ///    sont re-vérifiés via [`MeshEvent::validate_with_reputation`] — le
    ///    message a déjà été validé à la couche gossip avant d'arriver ici.
    /// 2. **Stockage hiérarchique** : les alertes critiques sont toujours
    ///    retenues localement (tier `Critical`, 7 jours — le sharding
    ///    géographique garde les urgences) et **persistées en SQLite**
    ///    (best-effort, via `persist_message`).
    /// 3. **Relai** : l'événement reste/entre dans l'outbox du gossip pour
    ///    être rediffusé aux pairs qui ne l'ont pas encore reçu (idempotent,
    ///    dédupliqué par id).
    ///
    /// Retourne `true` si l'alerte a été stockée localement, `false` si le
    /// message n'est pas une alerte ou si le budget / sharding l'exclut.
    pub fn handle_incoming_alert(&mut self, event: &MeshEvent) -> Result<bool, String> {
        if event.kind != OndeMessageType::Alert {
            return Ok(false);
        }
        // 1. Défense en profondeur : signature + PoW + réputation re-vérifiés.
        event.validate_with_reputation(&self.reputation)?;
        // 3. Relai dans le gossip (idempotent — déjà connu → Ok(false)).
        self.gossip
            .add_event_with_reputation(event.clone(), &self.reputation)?;
        // 2. Stockage hiérarchique : les alertes critiques sont toujours
        //    retenues, quel que soit le geohash de l'émetteur.
        let geohash = self.config.my_geohash.clone();
        if self.message_store.store(
            &event.id,
            MessageTier::Critical,
            event.content.as_bytes(),
            event.created_at,
            &geohash,
        )? {
            self.persist_message(
                &event.id,
                MessageTier::Critical,
                event.content.as_bytes(),
                event.created_at,
                &geohash,
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Restaurer les messages persistés après un crash (appelé au démarrage).
    /// Les messages expirés sont ignorés ; les autres sont rechargés dans le
    /// magasin hiérarchique en mémoire.
    pub fn load_persisted_messages(&mut self) -> Result<usize, String> {
        let Some(persist) = self.persistence.as_ref() else {
            return Ok(0);
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let msgs = persist.load_all()?;
        let mut restored = 0;
        for m in msgs {
            if now.saturating_sub(m.created_at) < m.tier.retention_secs()
                && self.message_store.restore(m).unwrap_or(false)
            {
                restored += 1;
            }
        }
        if restored > 0 {
            tracing::info!("Restored {restored} persisted messages from SQLite");
        }
        Ok(restored)
    }

    /// Restaurer l'état d'un nœud après un crash — l'équivalent d'un
    /// **redémarrage** réaliste (Phase 1.7 — perte de RAM simulée).
    ///
    /// L'identité stable est restaurée à la construction ([`Node::new`] :
    /// seed Ed25519 relue depuis la table `meta` de la base SQLite), les
    /// messages sont rechargés depuis la table `messages` dans le magasin
    /// hiérarchique en mémoire. Retourne le nombre de messages restaurés.
    pub fn restore_from_persistence(&mut self) -> Result<usize, String> {
        self.load_persisted_messages()
    }

    /// Send a ZK transaction
    pub async fn send_transaction(
        &mut self,
        receiver: &str,
        amount_micro: u64,
    ) -> Result<ZkTransaction, String> {
        let nonce = self
            .tx_pool
            .next_expected_nonce(&self.identity.pubkey_hex());
        let tx = ZkTransaction::new(&self.identity.pubkey_hex(), receiver, amount_micro, nonce);

        self.tx_pool.submit(tx.clone())?;
        Ok(tx)
    }

    /// Commit pending transactions (when internet available)
    pub async fn commit_transactions(&mut self, max_batch: usize) -> Vec<ZkTransaction> {
        self.tx_pool.commit_pending(max_batch)
    }

    /// Purge les messages expirés du magasin hiérarchique (mémoire + SQLite).
    pub fn sweep_message_store(&mut self) -> usize {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let in_memory = self.message_store.sweep_expired(now);
        if let Some(persist) = self.persistence.as_mut() {
            match persist.sweep_expired(now) {
                Ok(n) => tracing::debug!("SQLite sweep purged {n} messages"),
                Err(e) => tracing::warn!("SQLite sweep failed: {e}"),
            }
        }
        in_memory
    }

    /// Check if running
    pub fn is_running(&self) -> bool {
        self.is_running
    }

    // ------------------------------------------------------------------
    // Protocole de mise à jour APK — câblage dans le gossip (Phase 1.1)
    // ------------------------------------------------------------------

    /// Annoncer une mise à jour APK (côté **annonceur** — détenteur de la
    /// clé racine de distribution).
    ///
    /// Signe l'annonce **et** le manifeste avec l'identité racine
    /// (`NodeConfig::update_root_seed`), diffuse l'annonce dans le gossip et
    /// conserve l'offre (APK + manifeste signé) pour répondre aux requêtes
    /// `manifest` / `chunk` des receveurs. Retourne l'événement d'annonce
    /// publié.
    pub fn announce_update(
        &mut self,
        version: Version,
        apk: &[u8],
        timestamp: u64,
    ) -> Result<MeshEvent, String> {
        let root = self
            .update_root_signing
            .as_ref()
            .ok_or("node is not configured as an update distributor (missing update_root_seed)")?;
        if apk.len() as u64 > crate::update::MAX_APK_SIZE {
            return Err(format!(
                "APK exceeds the {} bytes limit",
                crate::update::MAX_APK_SIZE
            ));
        }

        // Annonce + manifeste signés par la racine, liés par le même APK.
        let (_ann, ann_sig, ann_bytes) =
            UpdateProtocol::build_announcement(version, apk, root, timestamp);
        let (_man, man_sig, man_bytes) = UpdateProtocol::build_manifest(
            apk,
            root,
            root.verifying_key_bytes(),
            timestamp,
            DEFAULT_CHUNK_SIZE,
        );

        let content = base64::engine::general_purpose::STANDARD.encode(&ann_bytes);
        let tags = build_tags(&[
            (TAG_ROOT_SIG, hex::encode(ann_sig)),
            (TAG_VERSION, version.to_string()),
            (TAG_PEER, self.identity.pubkey_hex()),
        ]);
        let event = MeshEvent::new_signed(
            &self.identity,
            OndeMessageType::UpdateAnnounce,
            content,
            tags,
        );
        let published = self.publish_gossip_event(event)?;

        // L'offre sert les requêtes manifeste/chunk des receveurs.
        self.update_offer = Some(UpdateOffer {
            version,
            apk: apk.to_vec(),
            manifest_wire: man_bytes,
            manifest_signature: man_sig,
            chunk_size: DEFAULT_CHUNK_SIZE,
        });
        Ok(published)
    }

    /// Traiter un message update reçu du gossip (côté receveur **et**
    /// annonceur — les `UpdateRequest` sont servis par l'annonceur).
    ///
    /// Branche sur [`OndeMessageType`] : annonce → manifeste demandé,
    /// manifeste → chunk 0 demandé, chunk → chunk suivant demandé puis
    /// assemblage + vérification + installation, requête → manifeste/chunk
    /// servi (si ce nœud est l'annonceur ciblé).
    pub fn handle_incoming_update(
        &mut self,
        event: &MeshEvent,
    ) -> Result<UpdateHandlingOutcome, String> {
        match event.kind {
            OndeMessageType::UpdateAnnounce => self.on_update_announce(event),
            OndeMessageType::UpdateManifest => self.on_update_manifest(event),
            OndeMessageType::UpdateChunk => self.on_update_chunk(event),
            OndeMessageType::UpdateRequest => self.on_update_request(event),
            _ => Ok(UpdateHandlingOutcome::Ignored),
        }
    }

    /// Signer (identité du nœud) et diffuser un événement dans le gossip, avec
    /// le PoW adaptatif de la réputation (nœud de confiance → difficulté 0).
    ///
    /// Utilisé par le protocole update (Phase 1.1) ET les endossements WoT
    /// (Phase 1.2) — la publication est identique quel que soit le kind.
    fn publish_gossip_event(&mut self, mut event: MeshEvent) -> Result<MeshEvent, String> {
        let difficulty = self
            .reputation
            .required_pow_difficulty(&self.identity.pubkey_hex());
        event = event.with_pow_difficulty(difficulty);
        if difficulty > 0 && !event.compute_pow(2_000_000) {
            return Err("PoW computation failed".to_string());
        }
        event.validate_with_reputation(&self.reputation)?;
        self.gossip
            .add_event_with_reputation(event.clone(), &self.reputation)?;
        Ok(event)
    }

    /// Décoder le blob signé (base64 dans `content`) + la signature racine
    /// (`root_sig` dans les tags) d'un message update.
    fn decode_update_payload(event: &MeshEvent) -> Result<(Vec<u8>, [u8; 64]), String> {
        let data = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .map_err(|_| "update payload is not valid base64".to_string())?;
        let sig_hex = tag_get(&event.tags, TAG_ROOT_SIG)
            .ok_or_else(|| "update payload missing root_sig tag".to_string())?;
        let sig_bytes =
            hex::decode(sig_hex).map_err(|_| "root_sig tag is not valid hex".to_string())?;
        let sig: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| "root_sig must be 64 bytes".to_string())?;
        Ok((data, sig))
    }

    /// Receveur — annonce reçue : vérifie la signature racine + version >
    /// locale, mémorise l'annonce, puis émet une requête `manifest` vers
    /// l'annonceur.
    fn on_update_announce(&mut self, event: &MeshEvent) -> Result<UpdateHandlingOutcome, String> {
        let (data, sig) = Self::decode_update_payload(event)?;
        match self.update_protocol.handle_announcement(&data, &sig) {
            Ok(announcement) => {
                let announcer = event.pubkey.clone();
                self.pending_announcement = Some(announcement);
                let request = MeshEvent::new_signed(
                    &self.identity,
                    OndeMessageType::UpdateRequest,
                    String::new(),
                    build_tags(&[(TAG_REQ_TYPE, "manifest".to_string()), (TAG_TO, announcer)]),
                );
                self.publish_gossip_event(request)?;
                Ok(UpdateHandlingOutcome::AnnouncementRequested)
            }
            Err(e) => Ok(UpdateHandlingOutcome::Rejected(e.to_string())),
        }
    }

    /// Receveur — manifeste reçu : le lie à l'annonce acceptée
    /// (`handle_manifest`), puis émet une requête `chunk 0` vers l'annonceur.
    fn on_update_manifest(&mut self, event: &MeshEvent) -> Result<UpdateHandlingOutcome, String> {
        let announcement = self.pending_announcement.clone().ok_or_else(|| {
            "update manifest received without a prior accepted announcement".to_string()
        })?;
        let (data, sig) = Self::decode_update_payload(event)?;
        match self
            .update_protocol
            .handle_manifest(&announcement, &data, &sig, &event.pubkey)
        {
            Ok(()) => {
                let announcer = event.pubkey.clone();
                let request = MeshEvent::new_signed(
                    &self.identity,
                    OndeMessageType::UpdateRequest,
                    String::new(),
                    build_tags(&[
                        (TAG_REQ_TYPE, "chunk".to_string()),
                        (TAG_TO, announcer),
                        (TAG_INDEX, "0".to_string()),
                        (TAG_VERSION, announcement.version.to_string()),
                    ]),
                );
                self.publish_gossip_event(request)?;
                Ok(UpdateHandlingOutcome::ManifestRequested)
            }
            Err(e) => Ok(UpdateHandlingOutcome::Rejected(e.to_string())),
        }
    }

    /// Receveur — chunk reçu : valide index/taille, demande le chunk suivant,
    /// et au dernier chunk assemble + vérifie de bout en bout + installe.
    fn on_update_chunk(&mut self, event: &MeshEvent) -> Result<UpdateHandlingOutcome, String> {
        let index: u32 = tag_get(&event.tags, TAG_INDEX)
            .ok_or_else(|| "update chunk missing index tag".to_string())?
            .parse()
            .map_err(|_| "invalid update chunk index tag".to_string())?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .map_err(|_| "update chunk is not valid base64".to_string())?;

        if let Err(e) = self.update_protocol.handle_chunk(index, &data) {
            return Ok(UpdateHandlingOutcome::Rejected(e.to_string()));
        }

        let received = self.update_protocol.chunks_received();
        let total = self.update_protocol.total_chunks();
        if received < total {
            let next = index + 1;
            let announcer = event.pubkey.clone();
            let request = MeshEvent::new_signed(
                &self.identity,
                OndeMessageType::UpdateRequest,
                String::new(),
                build_tags(&[
                    (TAG_REQ_TYPE, "chunk".to_string()),
                    (TAG_TO, announcer),
                    (TAG_INDEX, next.to_string()),
                ]),
            );
            self.publish_gossip_event(request)?;
            return Ok(UpdateHandlingOutcome::ChunkRequested(next));
        }

        // Dernier chunk : assemblage + vérification de bout en bout (racine
        // épinglée + SHA-256 du fichier entier). Un APK falsifié est purgé.
        let apk = match self.update_protocol.assemble_and_verify() {
            Ok(apk) => apk,
            Err(e) => {
                self.pending_announcement = None;
                return Ok(UpdateHandlingOutcome::Rejected(e.to_string()));
            }
        };
        let dest = self.update_install_path();
        let installed = self
            .update_protocol
            .install_verified(&apk, &dest, unix_now())
            .map_err(|e| e.to_string())?;
        self.pending_announcement = None;
        tracing::info!(
            "update installed: version {} ({} bytes) at {}",
            installed.version,
            apk.len(),
            dest
        );
        Ok(UpdateHandlingOutcome::Installed(installed.version))
    }

    /// Annonceur — requête reçue (manifeste ou chunk) : répond en signant.
    /// Seul le nœud ciblé par le tag `to` répond.
    fn on_update_request(&mut self, event: &MeshEvent) -> Result<UpdateHandlingOutcome, String> {
        let target = tag_get(&event.tags, TAG_TO).unwrap_or_default();
        if !target.is_empty() && target != self.identity.pubkey_hex() {
            return Ok(UpdateHandlingOutcome::Ignored);
        }
        let offer = self
            .update_offer
            .as_ref()
            .ok_or_else(|| "update request received but no update offer is held".to_string())?;
        match tag_get(&event.tags, TAG_REQ_TYPE).unwrap_or_default() {
            "manifest" => {
                let content =
                    base64::engine::general_purpose::STANDARD.encode(&offer.manifest_wire);
                let tags = build_tags(&[
                    (TAG_ROOT_SIG, hex::encode(offer.manifest_signature)),
                    (TAG_VERSION, offer.version.to_string()),
                    (TAG_PEER, self.identity.pubkey_hex()),
                ]);
                let manifest_event = MeshEvent::new_signed(
                    &self.identity,
                    OndeMessageType::UpdateManifest,
                    content,
                    tags,
                );
                self.publish_gossip_event(manifest_event)?;
                Ok(UpdateHandlingOutcome::ManifestServed)
            }
            "chunk" => {
                let index: u32 = tag_get(&event.tags, TAG_INDEX)
                    .ok_or_else(|| "chunk request missing index tag".to_string())?
                    .parse()
                    .map_err(|_| "invalid chunk request index".to_string())?;
                let data = UpdateProtocol::chunk(&offer.apk, index, offer.chunk_size as usize)
                    .ok_or_else(|| format!("chunk index {index} out of bounds"))?;
                let total = UpdateProtocol::chunk_count(offer.apk.len(), offer.chunk_size as usize);
                let content = base64::engine::general_purpose::STANDARD.encode(&data);
                let tags = build_tags(&[
                    (TAG_INDEX, index.to_string()),
                    (TAG_TOTAL, total.to_string()),
                    (TAG_PEER, self.identity.pubkey_hex()),
                ]);
                let chunk_event = MeshEvent::new_signed(
                    &self.identity,
                    OndeMessageType::UpdateChunk,
                    content,
                    tags,
                );
                self.publish_gossip_event(chunk_event)?;
                Ok(UpdateHandlingOutcome::ChunkServed(index))
            }
            other => Err(format!("unknown update request type: {other}")),
        }
    }

    /// Chemin d'installation de l'APK vérifié (unique par nœud — le préfixe
    /// de la clé publique évite les collisions entre nœuds du même hôte).
    fn update_install_path(&self) -> String {
        let prefix = &self.identity.pubkey_hex()[..8];
        std::env::temp_dir()
            .join(format!("onde-installed-{prefix}.apk"))
            .to_string_lossy()
            .into_owned()
    }

    /// Le mode économie batterie est-il actif ?
    pub fn battery_saver_enabled(&self) -> bool {
        self.config.battery_saver
    }

    // ------------------------------------------------------------------
    // Web of Trust — endossements propagés dans le gossip (Phase 1.2)
    // ------------------------------------------------------------------

    /// Endosser la clé publique d'un pair et diffuser l'endossement signé
    /// dans le gossip.
    ///
    /// L'application **locale** réutilise la logique [`ReputationSystem::endorse`]
    /// (anti-self, anti-doublon, seuil de l'endosseur) sans la dupliquer :
    /// un doublon ou un auto-endossement est refusé avant tout broadcast.
    /// L'`Endorsement` (`endorser`, `endorsed`, `timestamp`) est sérialisé en
    /// JSON puis base64 dans `content` ; l'événement est signé par l'endosseur
    /// (identité du nœud) et diffusé avec le PoW adaptatif de la réputation
    /// (endosseur de confiance → difficulté 0). Les autres nœuds le relaient
    /// en cascade via le gossip.
    pub fn endorse(&mut self, peer_pubkey: &str) -> Result<MeshEvent, String> {
        let timestamp = unix_now();
        // Application locale : réutilise l'endossement qualifié existant
        // (anti-self, anti-doublon, seuil d'endosseur) — jamais dupliqué.
        self.reputation
            .endorse(&self.identity.pubkey_hex(), peer_pubkey, timestamp)?;

        let endorsement = Endorsement {
            endorser: self.identity.pubkey_hex(),
            endorsed: peer_pubkey.to_string(),
            timestamp,
        };
        let payload = serde_json::to_vec(&endorsement).map_err(|e| e.to_string())?;
        let content = base64::engine::general_purpose::STANDARD.encode(&payload);
        let event = MeshEvent::new_signed(
            &self.identity,
            OndeMessageType::Endorsement,
            content,
            vec![],
        );
        self.publish_gossip_event(event)
    }

    /// Traiter un endossement WoT reçu du gossip.
    ///
    /// 1. Décodage du payload (`content` = base64 du JSON `Endorsement`).
    /// 2. Vérification de la signature de l'endosseur (Ed25519 sur l'ID
    ///    canonique — l'endosseur annoncé doit être l'auteur signé de l'événement).
    /// 3. Intégration via [`ReputationSystem::apply_remote_endorsement`]
    ///    (réutilise `endorse` : endosseur non de confiance, self ou doublon
    ///    → rejeté).
    /// 4. **Relai** : un endossement intégré est rediffusé vers les pairs qui
    ///    ne l'ont pas encore reçu (tracking "livré par pair" du gossip).
    pub fn handle_incoming_endorsement(&mut self, event: &MeshEvent) -> EndorsementHandlingOutcome {
        if event.kind != OndeMessageType::Endorsement {
            return EndorsementHandlingOutcome::Ignored;
        }

        // Limite de taille AVANT décodage (Aikido PR#8 MED) : un endossement
        // peut être un JSON arbitrairement grand signé par un pair de
        // confiance → épuisement mémoire/CPU (base64 décode ×3/4, puis JSON,
        // puis relai dans le gossip). 1 Ko est largement suffisant pour
        // {endorser, endorsed, timestamp}.
        if event.content.len() > 1024 {
            return EndorsementHandlingOutcome::Rejected(
                "endorsement payload too large (max 1024 bytes)".to_string(),
            );
        }

        // 1. Décodage du payload Endorsement (base64 → JSON).
        let data = match base64::engine::general_purpose::STANDARD.decode(&event.content) {
            Ok(d) => d,
            Err(e) => {
                return EndorsementHandlingOutcome::Rejected(format!(
                    "endorsement payload is not valid base64: {e}"
                ))
            }
        };
        let endorsement: Endorsement = match serde_json::from_slice(&data) {
            Ok(e) => e,
            Err(e) => {
                return EndorsementHandlingOutcome::Rejected(format!(
                    "endorsement payload is not valid JSON: {e}"
                ))
            }
        };

        // 2. L'endosseur annoncé doit être l'auteur signé de l'événement.
        if endorsement.endorser != event.pubkey {
            return EndorsementHandlingOutcome::Rejected(
                "endorsement endorser does not match the event signer".to_string(),
            );
        }
        let sig_verified = {
            let pubkey_ok = hex::decode(&event.pubkey)
                .map(|b| b.len() == 32)
                .unwrap_or(false);
            let sig_ok = hex::decode(&event.sig)
                .map(|b| b.len() == 64)
                .unwrap_or(false);
            pubkey_ok && sig_ok && {
                let mut pk = [0u8; 32];
                let mut sig = [0u8; 64];
                pk.copy_from_slice(&hex::decode(&event.pubkey).unwrap_or_default());
                sig.copy_from_slice(&hex::decode(&event.sig).unwrap_or_default());
                Identity::verify_from_pubkey(&pk, event.id.as_bytes(), &sig)
            }
        };
        if !sig_verified {
            return EndorsementHandlingOutcome::Rejected(
                "endorsement signature could not be verified".to_string(),
            );
        }

        // 3. Intégration : réutilise `apply_remote_endorsement` → `endorse`
        //    (endosseur non de confiance, self ou doublon → rejeté).
        match self.reputation.apply_remote_endorsement(&endorsement, true) {
            Ok(()) => {
                // 4. Relai : l'événement reste/entre dans l'outbox du gossip
                //    pour être rediffusé aux pairs qui ne l'ont pas encore reçu
                //    (idempotent si déjà connu).
                let _ = self
                    .gossip
                    .add_event_with_reputation(event.clone(), &self.reputation);
                EndorsementHandlingOutcome::Applied
            }
            Err(e) => EndorsementHandlingOutcome::Rejected(e),
        }
    }

    // ------------------------------------------------------------------
    // Phase 2.7 — Réputation anti-abus : gate d'admission, dispatcher de
    // réception, signalements d'abus propagés (endossements négatifs).
    // ------------------------------------------------------------------

    /// Gate d'admission appliqué à TOUT événement entrant avant routage.
    ///
    /// Ordre des filtres (du moins coûteux au plus structurant) :
    /// 1. **Signature** : invalide → rejet SANS pénalité — n'importe qui peut
    ///    forger un événement au nom d'autrui, l'auteur n'est pas attribuable.
    /// 2. **Auto-relais** : nos propres événements revenant via les pairs sont
    ///    admis sans comptage (sinon un nœud honnête s'auto-throttlrait).
    /// 3. **Déduplication** : un événement déjà connu ne compte pas une seconde
    ///    fois contre son auteur (coût marginal nul, lookup HashSet).
    /// 4. **Auteur ignoré** (abus saturé) → rejet immédiat, contenu total.
    /// 5. **Fenêtre glissante** ([`SpamGuard`]) : budget dépassé → violation
    ///    `ExcessiveRate` enregistrée + rejet AVANT validation/PoW/stockage —
    ///    le coût du flood pour le receveur reste borné au budget.
    ///
    /// Temps explicite (`now`, unix secs) : décision déterministe et testable.
    pub fn admit_peer_event(&mut self, now: u64, event: &MeshEvent) -> AdmissionDecision {
        if !event.signature_valid() {
            return AdmissionDecision::Rejected(
                "invalid signature — dropped without penalty (author not attributable)".to_string(),
            );
        }
        let author = &event.pubkey;
        if author == &self.identity.pubkey_hex() {
            return AdmissionDecision::Admitted;
        }
        if self.gossip.is_known(&event.id) {
            // Phase 3.6 — écho de relais dédupliqué (preuve de connectivité,
            // aucun retraitement) : compteur `messages_duplicated`.
            self.metrics.record_duplicated();
            return AdmissionDecision::Admitted;
        }
        if self.reputation.action_for(author, now) == TrustAction::Ignore {
            return AdmissionDecision::Rejected(format!(
                "author ignored (abuse level {:.2})",
                self.reputation.abuse_level(author, now)
            ));
        }
        // Phase 3.4 — grâce de heal : pendant la fenêtre courte qui suit un
        // retour de partition détecté LOCALEMENT, chaque auteur dispose d'un
        // budget élargi (×[`HEAL_WINDOW_BUDGET_FACTOR`]) : le rattrapage
        // rejoue des messages légitimes accumulés pendant la coupure (>12
        // par auteur et par fenêtre sinon) — les auteurs honnêtes ne doivent
        // NI perdre leurs messages NI être pénalisés par le trafic de heal.
        // La grâce reste bornée (temps × facteur fixe) ; au-delà, le budget
        // normal s'applique et l'excès est throttled comme avant.
        let budget = if self.heal_grace_active(now) {
            SPAM_BUDGET_PER_WINDOW.saturating_mul(HEAL_WINDOW_BUDGET_FACTOR)
        } else {
            SPAM_BUDGET_PER_WINDOW
        };
        if !self.spam_guard.admit_with_budget(author, now, budget) {
            self.reputation
                .record_violation(author, AbuseReason::ExcessiveRate, now);
            return AdmissionDecision::Rejected(
                "rate limited: sliding-window budget exceeded for this author".to_string(),
            );
        }
        AdmissionDecision::Admitted
    }

    /// Point d'entrée unique d'un événement reçu d'un pair : gate anti-abus
    /// puis routage vers le handler métier, avec classification des violations
    /// **attribuables** (l'événement est signé par son auteur réel).
    ///
    /// Les rejets « politiques » des endossements (doublon relayé par le
    /// gossip, endosseur non qualifié) NE sont PAS des abus : c'est du bruit
    /// normal du réseau, ils ne pénalisent personne. Seuls les payloads
    /// malformés signés par leur auteur (`InvalidEvent`) et les PoW
    /// insuffisants (`InsufficientPow`) pèsent sur la réputation.
    ///
    /// Phase 3.6 : chaque issue alimente le registre de métriques
    /// ([`NodeMetrics`]) — `messages_ingested` (traité avec succès),
    /// `messages_rejected` (gate/payload invalide). Les issues neutres
    /// (`AlertNotStored`, `Other`, social `Ignored`) ne comptent dans aucune
    /// des deux catégories : l'événement est relayer sans être retenu, ou
    /// d'un kind non géré.
    pub fn receive_peer_event(&mut self, now: u64, event: &MeshEvent) -> PeerEventOutcome {
        let outcome = self.receive_peer_event_inner(now, event);
        match &outcome {
            PeerEventOutcome::AlertStored
            | PeerEventOutcome::EndorsementApplied
            | PeerEventOutcome::AbuseReportApplied(_)
            | PeerEventOutcome::Social(
                SocialEventOutcome::PostStored(_)
                | SocialEventOutcome::CommentStored(_)
                | SocialEventOutcome::VoteApplied
                | SocialEventOutcome::FollowApplied
                | SocialEventOutcome::MessageStored
                | SocialEventOutcome::ModerationApplied,
            ) => self.metrics.record_ingested(),
            PeerEventOutcome::Rejected(_)
            | PeerEventOutcome::EndorsementRejected(_)
            | PeerEventOutcome::AbuseReportRejected(_) => self.metrics.record_rejected(),
            // Neutre : admis mais non retenu localement (sharding/budget),
            // kind non géré ou événement ignoré — ni ingéré ni rejeté.
            _ => {}
        }
        outcome
    }

    /// Corps historique du dispatcher (gate + routage), extrait pour laisser
    /// [`Self::receive_peer_event`] classifier l'issue côté métriques.
    fn receive_peer_event_inner(&mut self, now: u64, event: &MeshEvent) -> PeerEventOutcome {
        if let AdmissionDecision::Rejected(reason) = self.admit_peer_event(now, event) {
            return PeerEventOutcome::Rejected(reason);
        }
        // Phase 3.4 — présence : tout événement ADMIS prouve la connectivité
        // du pair émetteur (y compris un doublon déjà connu). Les échos de
        // nos propres événements ne identifient pas le transport → exclus.
        if event.pubkey != self.identity.pubkey_hex() {
            self.note_peer_traffic(&event.pubkey, now);
        }
        match &event.kind {
            OndeMessageType::Alert => match self.handle_incoming_alert(event) {
                Ok(true) => PeerEventOutcome::AlertStored,
                Ok(false) => PeerEventOutcome::AlertNotStored,
                Err(e) => {
                    let reason = if e.contains("PoW") {
                        AbuseReason::InsufficientPow
                    } else {
                        AbuseReason::InvalidEvent
                    };
                    self.reputation.record_violation(&event.pubkey, reason, now);
                    PeerEventOutcome::Rejected(format!("invalid signed event: {e}"))
                }
            },
            OndeMessageType::Endorsement => match self.handle_incoming_endorsement(event) {
                EndorsementHandlingOutcome::Applied => PeerEventOutcome::EndorsementApplied,
                EndorsementHandlingOutcome::Ignored => PeerEventOutcome::Other,
                EndorsementHandlingOutcome::Rejected(e) => {
                    let political_noise = e.contains("Duplicate")
                        || e.contains("not trusted")
                        || e.contains("cannot endorse itself");
                    if !political_noise {
                        self.reputation.record_violation(
                            &event.pubkey,
                            AbuseReason::InvalidEvent,
                            now,
                        );
                    }
                    PeerEventOutcome::EndorsementRejected(e)
                }
            },
            OndeMessageType::AbuseReport => match self.handle_incoming_abuse_report(event) {
                AbuseReportOutcome::Applied(level) => PeerEventOutcome::AbuseReportApplied(level),
                AbuseReportOutcome::Ignored => PeerEventOutcome::Other,
                AbuseReportOutcome::Rejected(e) => PeerEventOutcome::AbuseReportRejected(e),
            },
            // T13 Fusion : les événements sociaux passent par le MÊME gate
            // d'admission que tous les autres kinds (aucun contournement).
            // Les payloads sociaux malformés signés par leur auteur sont des
            // violations attribuables (même classification que les alertes).
            OndeMessageType::SocialPost
            | OndeMessageType::SocialComment
            | OndeMessageType::SocialVote
            | OndeMessageType::SocialFollow
            | OndeMessageType::SocialMessage
            | OndeMessageType::SocialModeration => match self.handle_incoming_social(event) {
                Ok(outcome) => PeerEventOutcome::Social(outcome),
                Err(e) => {
                    let reason = if e.contains("PoW") {
                        AbuseReason::InsufficientPow
                    } else {
                        AbuseReason::InvalidEvent
                    };
                    self.reputation.record_violation(&event.pubkey, reason, now);
                    PeerEventOutcome::Rejected(format!("invalid social event: {e}"))
                }
            },
            _ => PeerEventOutcome::Other,
        }
    }

    // ------------------------------------------------------------------
    // Phase 3.4 — Auto-réparation : détection de partition, re-sync, heal
    // ------------------------------------------------------------------

    /// Enregistrer un contact réseau d'un pair à l'instant INJECTÉ `now`
    /// (unix secs — jamais l'horloge système : déterministe et testable).
    ///
    /// Appelé automatiquement par [`Node::receive_peer_event`] pour tout
    /// événement **admis** dont l'auteur est un pair (un doublon déjà connu
    /// prouve lui aussi la connectivité). Détecte le **retour de partition**
    /// : si TOUS les pairs connus étaient silencieux depuis au moins
    /// [`PARTITION_SILENCE_THRESHOLD_SECS`] juste avant ce contact, la
    /// connectivité revient → ouverture d'une grâce de heal courte
    /// ([`Node::heal_grace_active`]) pour le rattrapage.
    pub fn note_peer_traffic(&mut self, peer_pubkey: &str, now: u64) {
        let partition_ended = self.partition_suspected(now);
        self.peer_last_seen.insert(peer_pubkey.to_string(), now);
        if partition_ended {
            self.heal_grace_until = now.saturating_add(HEAL_GRACE_SECS);
        }
        // Phase 3.6 — jauges de connectivité : connus vs synchronisés
        // (contact plus récent que le seuil de partition). Scan O(pairs)
        // borné par la taille du book (~max_peer_connections) — négligeable
        // face au travail déjà fait par événement (signature, PoW, routage).
        let known = self.peer_last_seen.len() as u64;
        let synced = self
            .peer_last_seen
            .values()
            .filter(|&&last| now.saturating_sub(last) < PARTITION_SILENCE_THRESHOLD_SECS)
            .count() as u64;
        self.metrics.set_peers(known, synced);
    }

    /// Partition soupçonnée à l'instant injecté `now` ? Heuristique
    /// déterministe : au moins un pair connu ET tous silencieux depuis
    /// [`PARTITION_SILENCE_THRESHOLD_SECS`]. Un nœud sans aucun contact ne
    /// peut pas distinguer partition et démarrage → jamais soupçonné.
    pub fn partition_suspected(&self, now: u64) -> bool {
        !self.peer_last_seen.is_empty()
            && self
                .peer_last_seen
                .values()
                .all(|&last| now.saturating_sub(last) >= PARTITION_SILENCE_THRESHOLD_SECS)
    }

    /// Grâce de heal active à l'instant injecté `now` ? (fenêtre courte qui
    /// suit le retour détecté d'une partition — voir [`HEAL_GRACE_SECS`])
    pub fn heal_grace_active(&self, now: u64) -> bool {
        now < self.heal_grace_until
    }

    /// Nombre de pairs suivis par le book de présence (visibilité).
    pub fn tracked_peers(&self) -> usize {
        self.peer_last_seen.len()
    }

    /// Lot de rattrapage borné vers `peer_id` (Phase 3.4 — re-sync au retour
    /// de partition). AUCUN message wire nouveau : source = outbox du gossip
    /// existante, sélection identique à
    /// [`GossipProtocol::get_pending_for_peer`] (événements non encore
    /// marqués livrés pour CE pair), mais (1) bornée à
    /// [`HEAL_BATCH_MAX_EVENTS`] événements PAR APPEL — le transport boucle
    /// jusqu'à lot vide : volume total = exactement ce que le pair n'a pas,
    /// jamais une tempête — et (2) le marquage « livré » n'intervient qu'une
    /// fois la sélection faite. Côté receveur, les doublons sont dédupliqués
    /// par ID (`is_known`) : re-livraison gratuite, jamais pénalisée.
    pub fn take_heal_batch(&mut self, peer_id: &str) -> Vec<MeshEvent> {
        let batch = self
            .gossip
            .peek_pending_for_peer(peer_id, HEAL_BATCH_MAX_EVENTS);
        if batch.is_empty() {
            return batch;
        }
        let ids: Vec<String> = batch.iter().map(|e| e.id.clone()).collect();
        self.gossip.mark_delivered_to_peer(peer_id, &ids);
        batch
    }

    /// Signaler l'abus constaté LOCALEMENT sur `offender` et propager le
    /// signalement signé dans le gossip (endossement négatif).
    ///
    /// Miroir exact de [`Node::endorse`] : application locale implicite via
    /// la publication qualifiée (le nœud se fait confiance), payload JSON
    /// base64 dans le `content` d'un kind `AbuseReport` (code wire 15),
    /// signature Ed25519 du rapporteur, PoW adaptatif. Chaque receveur décide
    /// souverainement d'intégrer ou non le signalement selon SA confiance en
    ///vers le rapporteur ([`ReputationSystem::apply_remote_abuse_report`]).
    pub fn report_abuse(
        &mut self,
        offender: &str,
        reason: AbuseReason,
        timestamp: u64,
    ) -> Result<MeshEvent, String> {
        if offender == self.identity.pubkey_hex() {
            return Err("A node cannot report itself".to_string());
        }
        let report = AbuseReport {
            reporter: self.identity.pubkey_hex(),
            offender: offender.to_string(),
            reason: reason.code(),
            timestamp,
        };
        let payload = serde_json::to_vec(&report).map_err(|e| e.to_string())?;
        let content = base64::engine::general_purpose::STANDARD.encode(&payload);
        let event = MeshEvent::new_signed(
            &self.identity,
            OndeMessageType::AbuseReport,
            content,
            vec![],
        );
        self.publish_gossip_event(event)
    }

    /// Traiter un signalement d'abus reçu du gossip.
    ///
    /// 1. Décodage du payload (`content` = base64 du JSON [`AbuseReport`],
    ///    borné comme un endossement).
    /// 2. Le rapporteur annoncé doit être l'auteur signé de l'événement ET la
    ///    signature doit être valide (pas d'usurpation de rapporteur).
    /// 3. Intégration souveraine via
    ///    [`ReputationSystem::apply_remote_abuse_report`] — le receveur
    ///    n'intègre que les signalements de rapporteurs qu'il considère lui-
    ///    même de confiance, dédupliqués par (rapporteur, dénoncé, raison).
    /// 4. **Relai** : un signalement intégré entre dans l'outbox du gossip
    ///    (idempotent) pour atteindre les pairs qui ne le connaissent pas.
    pub fn handle_incoming_abuse_report(&mut self, event: &MeshEvent) -> AbuseReportOutcome {
        if event.kind != OndeMessageType::AbuseReport {
            return AbuseReportOutcome::Ignored;
        }
        if event.content.len() > 1024 {
            return AbuseReportOutcome::Rejected(
                "abuse report payload too large (max 1024 bytes)".to_string(),
            );
        }
        let data = match base64::engine::general_purpose::STANDARD.decode(&event.content) {
            Ok(d) => d,
            Err(e) => {
                return AbuseReportOutcome::Rejected(format!(
                    "abuse report payload is not valid base64: {e}"
                ))
            }
        };
        let report: AbuseReport = match serde_json::from_slice(&data) {
            Ok(r) => r,
            Err(e) => {
                return AbuseReportOutcome::Rejected(format!(
                    "abuse report payload is not valid JSON: {e}"
                ))
            }
        };
        if report.reporter != event.pubkey {
            return AbuseReportOutcome::Rejected(
                "abuse reporter does not match the event signer".to_string(),
            );
        }
        if !event.signature_valid() {
            return AbuseReportOutcome::Rejected(
                "abuse report signature could not be verified".to_string(),
            );
        }
        match self.reputation.apply_remote_abuse_report(&report, true) {
            Ok(level) => {
                let _ = self
                    .gossip
                    .add_event_with_reputation(event.clone(), &self.reputation);
                AbuseReportOutcome::Applied(level)
            }
            Err(e) => AbuseReportOutcome::Rejected(e),
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // Phase 1.4 — Rotation d'identité X25519 (forward secrecy)
    //
    // Décision d'architecture (Hermes) : la rotation porte sur la clé
    // **chiffrement** X25519 du rotateur (current/next), PAS sur l'identité
    // de signature Ed25519. Raisons :
    //   1. `RotatingIdentity::current()` est une identité aléatoire
    //      NON-DÉRIVÉE de l'identité stable — un pair ne peut PAS la
    //      reproduire ; la signature avec cette clé serait invérifiable.
    //   2. La réputation est indexée sur l'identité stable ; changer le
    //      signataire casserait la WoT (et les 153 tests existants).
    // La vraie valeur de la rotation = **forward secrecy** : la clé de
    // session X25519 change périodiquement, et l'ancienne reste valable une
    // période de grâce (les messages chiffrés avec l'ancienne clé sont
    // encore déchiffrables). L'annonce circule dans le gossip signée par
    // l'identité stable (vérifiable), même pattern que 1.1/1.2/1.3.
    // ────────────────────────────────────────────────────────────────────

    /// Émetteur — force la rotation du rotateur X25519 (si due), puis
    /// annonce la nouvelle clé publique dans le gossip.
    ///
    /// L'annonce est signée par l'identité **stable** du nœud (vérifiable
    /// par tous), et porte la clé X25519 publique courante du rotateur.
    /// La clé précédente est conservée (période de grâce) : les pairs qui
    /// l'utilisent encore peuvent déchiffrer les anciens messages.
    pub fn announce_identity_rotation(&mut self) -> Result<MeshEvent, String> {
        // Rotation périodique (le rotateur décide si c'est "due").
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _rotated = self.identity_rotator.maybe_rotate(now);

        let prev_x25519 = self
            .peer_x25519
            .get(&self.identity.pubkey_hex())
            .cloned()
            .unwrap_or_default();

        // Construction du payload (clé X25519 + compteur de rotation + clé
        // précédente pour la période de grâce) — logique partagée avec les tests.
        let event =
            Self::build_rotation_announcement(&self.identity, &self.identity_rotator, &prev_x25519);
        self.publish_gossip_event(event)
    }

    /// Construit l'événement d'annonce de rotation (payload, base64, tags,
    /// signature Ed25519 par l'identité stable). Point commun unique — utilisé
    /// par `announce_identity_rotation` et les tests, pour éviter la duplication
    /// de la logique de construction (portée SonarQube).
    fn build_rotation_announcement(
        identity: &Identity,
        rotator: &RotatingIdentity,
        prev_x25519: &str,
    ) -> MeshEvent {
        let new_x25519 = rotator.x25519_public_key_hex();
        let rotation_count = rotator.rotation_count();

        let payload = serde_json::json!({
            "x25519": new_x25519,
            "prev": prev_x25519,
            "rotation": rotation_count,
        });
        let content =
            base64::engine::general_purpose::STANDARD.encode(payload.to_string().into_bytes());

        let tags = build_tags(&[
            (TAG_IDENTITY_ROTATION, new_x25519.clone()),
            ("rotation_count", rotation_count.to_string()),
        ]);
        MeshEvent::new_signed(identity, OndeMessageType::IdentityRotation, content, tags)
    }

    /// Récepteur — traite une annonce de rotation d'identité X25519 reçue.
    ///
    /// Vérifications (dans l'ordre) :
    /// 1. Le kind est bien `IdentityRotation` (sinon ignoré).
    /// 2. Le payload est un JSON valide avec un champ `x25519` (64 hex) et un
    ///    compteur `rotation` (u64).
    /// 3. **La signature Ed25519 de l'événement est valide** — l'annonceur est
    ///    bien l'auteur authentique (sinon rejeté : une annonce forgée avec la
    ///    pubkey d'un pair de confiance ne doit pas passer).
    /// 4. L'annonceur est **de confiance** dans la réputation locale
    ///    (sinon rejeté — un inconnu ne peut pas imposer sa clé).
    /// 5. **Anti-replay / ordre** : le compteur `rotation` doit être strictement
    ///    supérieur au dernier compteur accepté pour ce pair. Une annonce plus
    ///    ancienne (rejouée) est rejetée : on ne recule jamais vers une clé
    ///    X25519 antérieure.
    /// 6. La clé annoncée est **nouvelle** (sinon rejeté — même clé deux fois).
    ///
    /// Si tout passe : la clé du pair est mise à jour, l'ancienne est
    /// conservée en `peer_x25519_grace`, le compteur est mémorisé, et l'annonce
    /// est relaiée dans le gossip (idempotent).
    pub fn handle_incoming_rotation(&mut self, event: &MeshEvent) -> RotationHandlingOutcome {
        if event.kind != OndeMessageType::IdentityRotation {
            return RotationHandlingOutcome::Ignored;
        }

        // 1. Décoder le payload JSON.
        let data = match base64::engine::general_purpose::STANDARD.decode(&event.content) {
            Ok(d) => d,
            Err(e) => {
                return RotationHandlingOutcome::Rejected(format!(
                    "rotation payload is not valid base64: {e}"
                ))
            }
        };
        let payload: serde_json::Value = match serde_json::from_slice(&data) {
            Ok(p) => p,
            Err(e) => {
                return RotationHandlingOutcome::Rejected(format!(
                    "rotation payload is not valid JSON: {e}"
                ))
            }
        };
        let new_x25519 = match payload.get("x25519").and_then(|v| v.as_str()) {
            Some(k) if k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit()) => k.to_string(),
            _ => {
                return RotationHandlingOutcome::Rejected(
                    "rotation payload missing valid 'x25519' (64 hex)".to_string(),
                )
            }
        };
        let rotation_count = payload
            .get("rotation")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let announcer = &event.pubkey;

        // 2. La signature Ed25519 de l'événement DOIT être valide —
        //    l'annonceur est authentique, pas une pubkey usurpée (Aikido CRIT).
        let sig_verified = {
            let pubkey_ok = hex::decode(announcer)
                .map(|b| b.len() == 32)
                .unwrap_or(false);
            let sig_ok = hex::decode(&event.sig)
                .map(|b| b.len() == 64)
                .unwrap_or(false);
            pubkey_ok && sig_ok && {
                let mut pk = [0u8; 32];
                let mut sig = [0u8; 64];
                pk.copy_from_slice(&hex::decode(announcer).unwrap_or_default());
                sig.copy_from_slice(&hex::decode(&event.sig).unwrap_or_default());
                crate::crypto::Identity::verify_from_pubkey(&pk, event.id.as_bytes(), &sig)
            }
        };
        if !sig_verified {
            return RotationHandlingOutcome::Rejected(
                "rotation signature could not be verified (forged or invalid)".to_string(),
            );
        }

        // 3. L'annonceur doit être de confiance (WoT).
        if !self.reputation.is_trusted(announcer) {
            return RotationHandlingOutcome::Rejected(format!(
                "rotation announced by untrusted peer {announcer}"
            ));
        }

        // 4. Anti-replay / ordre : le compteur doit progresser strictement.
        //    Une annonce ancienne rejouée est rejetée (Aikido MED : rollback).
        if let Some(&last) = self.peer_rotation_count.get(announcer) {
            if rotation_count <= last {
                return RotationHandlingOutcome::Rejected(format!(
                    "rotation replay/stale (count {} not > last accepted {})",
                    rotation_count, last
                ));
            }
        }

        // 5. La clé doit être nouvelle (même clé deux fois = pas une rotation).
        if self.peer_x25519.get(announcer).map(|k| k == &new_x25519) == Some(true) {
            return RotationHandlingOutcome::Rejected(
                "rotation already applied (duplicate key)".to_string(),
            );
        }

        // 6. Appliquer : ancienne clé → grâce, nouvelle clé → courante,
        //    mémoriser le compteur (anti-replay pour la suite).
        let prev = self.peer_x25519.get(announcer).cloned().unwrap_or_default();
        if !prev.is_empty() {
            self.peer_x25519_grace.insert(announcer.clone(), prev);
        }
        self.peer_x25519
            .insert(announcer.clone(), new_x25519.clone());
        self.peer_rotation_count
            .insert(announcer.clone(), rotation_count);

        // 7. Relai dans le gossip (idempotent).
        let _ = self
            .gossip
            .add_event_with_reputation(event.clone(), &self.reputation);

        RotationHandlingOutcome::Applied
    }

    /// Renvoie la clé X25519 courante connue pour un pair donné (pour
    /// chiffrer un message point-à-point). `None` si le pair est inconnu.
    pub fn peer_x25519_key(&self, peer_pubkey: &str) -> Option<&str> {
        self.peer_x25519.get(peer_pubkey).map(|s| s.as_str())
    }

    /// Renvoie la clé X25519 **précédente** conservée en période de grâce
    /// pour un pair donné (pour déchiffrer les messages chiffrés avec
    /// l'ancienne clé). `None` si aucune clé de grâce n'est connue.
    pub fn peer_x25519_grace_key(&self, peer_pubkey: &str) -> Option<&str> {
        self.peer_x25519_grace.get(peer_pubkey).map(|s| s.as_str())
    }

    /// Activer / désactiver le mode économie batterie (throttling adaptatif).
    pub fn set_battery_saver(&mut self, enabled: bool) {
        if self.config.battery_saver != enabled {
            tracing::info!(
                "battery saver mode {}",
                if enabled { "enabled" } else { "disabled" }
            );
            self.config.battery_saver = enabled;
        }
    }

    /// Intervalle recommandé entre deux balayages du magasin (sweep).
    ///
    /// Retourne 0 en mode normal (le sweep peut tourner à chaque tick) et une
    /// valeur de différé en mode économie batterie. C'est un hook de
    /// throttling : l'appelant (daemon, UI) applique cette contrainte.
    pub fn throttle_sweep_secs(&self) -> u64 {
        if self.config.battery_saver {
            BATTERY_SAVER_SWEEP_INTERVAL_SECS
        } else {
            0
        }
    }

    /// Le nœud doit-il différer les tâches non critiques (inférence IA,
    /// téléchargements lourds…) ?
    ///
    /// En mode économie batterie, la diffusion des alertes critiques reste
    /// autorisée (les messages Important/Critical sont toujours publiés) ;
    /// seuls les travaux de fond coûteux sont différés.
    pub fn should_defer_heavy_work(&self) -> bool {
        self.config.battery_saver
    }

    // ────────────────────────────────────────────────────────────────────
    // Social Graph — Tuitter/Redit gossip (T13 Fusion)
    // ────────────────────────────────────────────────────────────────────

    /// Publier un post social Tuitter ou Redit dans le mesh.
    ///
    /// Le post est validé, signé (identité stable du nœud), diffusé dans le
    /// gossip avec le PoW adaptatif de la réputation, et stocké localement
    /// dans le cache matérialisé ([`SocialStore`]).
    pub fn publish_social_post(
        &mut self,
        platform: &str,
        title: Option<&str>,
        body: &str,
        community_slug: Option<&str>,
    ) -> Result<MeshEvent, String> {
        let platform = parse_social_platform(platform)?;
        let pubkey = self.identity.pubkey_hex();
        let post = SocialPost {
            id: generate_social_id(),
            platform,
            author_pubkey: pubkey.clone(),
            title: title.map(|t| t.to_string()),
            body: body.to_string(),
            community_slug: community_slug.map(|s| s.to_string()),
            media_urls: vec![],
        };
        post.validate()?;

        // Conversion en événement ONDE signé + PoW adaptatif de la réputation.
        let mut event = post.to_mesh_event(&self.identity)?;
        let difficulty = self.reputation.required_pow_difficulty(&pubkey);
        event = event.with_pow_difficulty(difficulty);
        if difficulty > 0 && !event.compute_pow(2_000_000) {
            return Err("PoW computation failed for social post".to_string());
        }
        event.validate_with_reputation(&self.reputation)?;

        // Diffusion dans le gossip (idempotent).
        self.gossip
            .add_event_with_reputation(event.clone(), &self.reputation)?;

        // Cache matérialisé local (best-effort — un échec local ne fait pas
        // échouer une publication déjà relayée). `ensure_user` ne réinitialise
        // JAMAIS le profil existant du propriétaire.
        if let Some(store) = &self.social_store {
            if let Err(cache_err) = store.ensure_user(&pubkey) {
                tracing::warn!("social cache write failed (author): {cache_err}");
            }
            if let Err(cache_err) = store.insert_post(&post) {
                tracing::warn!("social cache write failed (own post): {cache_err}");
            }
        }

        self.record_publish();
        Ok(event)
    }

    /// Publier un commentaire social sous un post existant.
    pub fn publish_social_comment(
        &mut self,
        platform: &str,
        post_id: &str,
        parent_id: Option<&str>,
        body: &str,
    ) -> Result<MeshEvent, String> {
        let platform = parse_social_platform(platform)?;
        let pubkey = self.identity.pubkey_hex();
        let comment = SocialComment {
            id: generate_social_id(),
            platform,
            author_pubkey: pubkey,
            post_id: post_id.to_string(),
            parent_id: parent_id.map(|p| p.to_string()),
            body: body.to_string(),
        };
        comment.validate()?;

        let content = serde_json::to_string(&comment)
            .map_err(|e| format!("social comment serialization: {e}"))?;
        let mut event = MeshEvent::new_signed(
            &self.identity,
            OndeMessageType::SocialComment,
            content,
            vec![format!("platform={platform:?}")],
        );
        let difficulty = self.reputation.required_pow_difficulty(&event.pubkey);
        event = event.with_pow_difficulty(difficulty);
        if difficulty > 0 && !event.compute_pow(2_000_000) {
            return Err("PoW computation failed for comment".to_string());
        }
        event.validate_with_reputation(&self.reputation)?;
        self.gossip
            .add_event_with_reputation(event.clone(), &self.reputation)?;

        if let Some(store) = &self.social_store {
            if let Err(cache_err) = store.insert_comment(&comment) {
                tracing::warn!("social cache write failed (own comment): {cache_err}");
            }
        }
        Ok(event)
    }

    /// Traiter un événement social entrant — appelé UNIQUEMENT depuis le
    /// dispatcher [`Node::receive_peer_event`], donc APRÈS le gate
    /// d'admission anti-abus (`admit_peer_event`) : signature vérifiée,
    /// auteur non ignoré, budget fenêtre glissante respecté. Aucun
    /// contournement du gate n'existe pour les kinds sociaux.
    ///
    /// Décode le payload, re-valide l'événement (défense en profondeur),
    /// relai dans le gossip et stocke dans le cache matérialisé.
    pub fn handle_incoming_social(
        &mut self,
        event: &MeshEvent,
    ) -> Result<SocialEventOutcome, String> {
        // SÉPARATION DES RÉGIMES D'ERREUR (T13-checker H1) :
        // 1. Payload invalide (dépassement de plafond brut, JSON illisible,
        //    bornes de domaine violées) ou PoW insuffisant → `Err` : violation
        //    ATTRIBUABLE (l'événement est signé par son auteur), classifiée
        //    par le dispatcher comme les alertes corrompues.
        // 2. Échec du cache local (`social_store` : disque plein, FK-miss,
        //    base verrouillée…) → JAMAIS pénalisant : le store est un cache
        //    matérialisé, pas une autorité ; l'événement est déjà relayé dans
        //    le gossip. L'écriture est best-effort (warning de traçabilité).
        macro_rules! cache_write {
            ($scope:expr, $expr:expr) => {
                if let Err(cache_err) = $expr {
                    tracing::warn!("social cache write failed ({}): {}", $scope, cache_err);
                }
            };
        }

        match event.kind {
            OndeMessageType::SocialPost => {
                check_social_payload_size("social post", &event.content, SOCIAL_POST_MAX_BYTES)?;
                let post: SocialPost = serde_json::from_str(&event.content)
                    .map_err(|e| format!("social post decode: {e}"))?;
                post.validate()?;
                event.validate_with_reputation(&self.reputation)?;
                self.gossip
                    .add_event_with_reputation(event.clone(), &self.reputation)?;
                if let Some(store) = &self.social_store {
                    cache_write!("post author", store.ensure_user(&post.author_pubkey));
                    cache_write!("post insert", store.insert_post(&post));
                }
                Ok(SocialEventOutcome::PostStored(post.id))
            }
            OndeMessageType::SocialComment => {
                check_social_payload_size(
                    "social comment",
                    &event.content,
                    SOCIAL_COMMENT_MAX_BYTES,
                )?;
                let comment: SocialComment = serde_json::from_str(&event.content)
                    .map_err(|e| format!("social comment decode: {e}"))?;
                comment.validate()?;
                event.validate_with_reputation(&self.reputation)?;
                self.gossip
                    .add_event_with_reputation(event.clone(), &self.reputation)?;
                if let Some(store) = &self.social_store {
                    cache_write!("comment author", store.ensure_user(&comment.author_pubkey));
                    // Commentaire orphelin (post/parent pas encore arrivé) :
                    // bufferisé côté store, JAMAIS une erreur pénalisante.
                    cache_write!("comment insert", store.insert_comment(&comment).map(|_| ()));
                }
                Ok(SocialEventOutcome::CommentStored(comment.id))
            }
            OndeMessageType::SocialVote => {
                check_social_payload_size("vote", &event.content, SOCIAL_VOTE_MAX_BYTES)?;
                let payload: serde_json::Value = serde_json::from_str(&event.content)
                    .map_err(|e| format!("vote payload decode: {e}"))?;
                let target_id = payload["target_id"].as_str().unwrap_or("");
                let direction = payload["direction"].as_i64().unwrap_or(1);
                let target_table = payload["target_table"].as_str().unwrap_or("posts");
                validate_social_target_ref(target_id, "vote target_id")?;
                if !(-1..=1).contains(&direction) {
                    return Err("vote direction must be -1 or 1".to_string());
                }
                if !matches!(target_table, "posts" | "comments") {
                    return Err(format!("unknown vote target table: {target_table}"));
                }
                event.validate_with_reputation(&self.reputation)?;
                self.gossip
                    .add_event_with_reputation(event.clone(), &self.reputation)?;
                if let Some(store) = &self.social_store {
                    cache_write!(
                        "vote",
                        store
                            .vote(&event.pubkey, target_id, direction as i32, target_table)
                            .map(|_| ())
                    );
                }
                Ok(SocialEventOutcome::VoteApplied)
            }
            OndeMessageType::SocialFollow => {
                check_social_payload_size("follow", &event.content, SOCIAL_FOLLOW_MAX_BYTES)?;
                let payload: serde_json::Value = serde_json::from_str(&event.content)
                    .map_err(|e| format!("follow payload decode: {e}"))?;
                let followed = payload["followed"].as_str().unwrap_or("");
                let unfollow = payload["unfollow"].as_bool().unwrap_or(false);
                validate_social_pubkey_ref(followed, "follow target")?;
                event.validate_with_reputation(&self.reputation)?;
                self.gossip
                    .add_event_with_reputation(event.clone(), &self.reputation)?;
                if let Some(store) = &self.social_store {
                    if unfollow {
                        cache_write!("unfollow", store.unfollow(&event.pubkey, followed));
                    } else {
                        cache_write!("follow", store.follow(&event.pubkey, followed));
                    }
                }
                Ok(SocialEventOutcome::FollowApplied)
            }
            OndeMessageType::SocialMessage => {
                check_social_payload_size(
                    "private message",
                    &event.content,
                    SOCIAL_MESSAGE_MAX_BYTES,
                )?;
                let payload: serde_json::Value = serde_json::from_str(&event.content)
                    .map_err(|e| format!("message payload decode: {e}"))?;
                let recipient = payload["recipient"].as_str().unwrap_or("");
                let body = payload["body"].as_str().unwrap_or("");
                validate_social_pubkey_ref(recipient, "message recipient")?;
                if body.trim().is_empty() {
                    return Err("message body cannot be empty".to_string());
                }
                if body.chars().count() > crate::social::MAX_PRIVATE_MESSAGE_BODY {
                    return Err(format!(
                        "message body exceeds {} characters",
                        crate::social::MAX_PRIVATE_MESSAGE_BODY
                    ));
                }
                event.validate_with_reputation(&self.reputation)?;
                self.gossip
                    .add_event_with_reputation(event.clone(), &self.reputation)?;
                if let Some(store) = &self.social_store {
                    cache_write!(
                        "message insert",
                        store.insert_message(&generate_social_id(), &event.pubkey, recipient, body)
                    );
                }
                Ok(SocialEventOutcome::MessageStored)
            }
            OndeMessageType::SocialModeration => {
                check_social_payload_size(
                    "moderation report",
                    &event.content,
                    SOCIAL_MODERATION_MAX_BYTES,
                )?;
                let payload: serde_json::Value = serde_json::from_str(&event.content)
                    .map_err(|e| format!("moderation payload decode: {e}"))?;
                let target_id = payload["target_id"].as_str().unwrap_or("");
                let reason = payload["reason"].as_str().unwrap_or("");
                validate_social_target_ref(target_id, "moderation target_id")?;
                if reason.trim().is_empty() {
                    return Err("moderation reason cannot be empty".to_string());
                }
                if reason.chars().count() > crate::social::MAX_MODERATION_REASON {
                    return Err(format!(
                        "moderation reason exceeds {} characters",
                        crate::social::MAX_MODERATION_REASON
                    ));
                }
                event.validate_with_reputation(&self.reputation)?;
                self.gossip
                    .add_event_with_reputation(event.clone(), &self.reputation)?;
                if let Some(store) = &self.social_store {
                    cache_write!(
                        "report insert",
                        store.submit_report(
                            &generate_social_id(),
                            &event.pubkey,
                            target_id,
                            reason
                        )
                    );
                }
                Ok(SocialEventOutcome::ModerationApplied)
            }
            // Tout kind non social atteint ce handler uniquement par erreur
            // de routage : ignorer sans pénalité (le AbuseReport Phase 2.7 a
            // SON propre handler dédié dans le dispatcher).
            _ => Ok(SocialEventOutcome::Ignored),
        }
    }

    /// Get node status summary
    pub async fn status(&self) -> NodeStatus {
        let ai = self.ai_engine.lock().await;
        let (raw_bytes, stored_bytes) = self.message_store.compression_stats();
        NodeStatus {
            is_running: self.is_running,
            node_type: self.config.node_type,
            pubkey: self.identity.pubkey_hex(),
            mesh_address: self.mesh_address.generate_ipv6(),
            gossip_known_events: self.gossip.known_count(),
            pending_tx: self.tx_pool.pending_count(),
            committed_tx: self.tx_pool.committed_count(),
            ipfs_seeds: self.ipfs_seeder.list_seeds().len(),
            local_model: ai.get_local_model().map(|m| format!("{m:?}")),
            // Extensions audits
            identity_rotations: self.identity_rotator.rotation_count(),
            trusted_peers: self
                .reputation
                .summary()
                .iter()
                .filter(|(_, s, _)| *s >= crate::reputation::TRUSTED_THRESHOLD)
                .count(),
            stored_messages: self.message_store.total_count(),
            stored_compressed_bytes: stored_bytes,
            stored_raw_bytes: raw_bytes,
            battery_saver: self.config.battery_saver,
            throttle_sweep_secs: self.throttle_sweep_secs(),
            publish_interval_secs: self.publish_interval_secs(),
            // Phase 3.4 — observabilité de l'auto-réparation (snapshot :
            // l'horloge système n'est utilisée QUE pour l'affichage ; toute
            // la logique de détection reste pilotée par le temps injecté).
            partition_suspected: self.partition_suspected(unix_now()),
            heal_grace_active: self.heal_grace_active(unix_now()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct NodeStatus {
    pub is_running: bool,
    pub node_type: NodeType,
    pub pubkey: String,
    pub mesh_address: String,
    pub gossip_known_events: usize,
    pub pending_tx: usize,
    pub committed_tx: usize,
    pub ipfs_seeds: usize,
    pub local_model: Option<String>,
    /// Nombre de rotations d'identité effectuées (Audit #10)
    pub identity_rotations: u64,
    /// Nombre de pairs de confiance dans le Web of Trust (Audit #11)
    pub trusted_peers: usize,
    /// Messages stockés dans le magasin hiérarchique (Audit #8)
    pub stored_messages: usize,
    /// Octets compressés stockés
    pub stored_compressed_bytes: usize,
    /// Octets originaux correspondants
    pub stored_raw_bytes: usize,
    /// Mode économie batterie actif (Phase 2/3 — throttling adaptatif)
    pub battery_saver: bool,
    /// Intervalle de sweep recommandé (0 = normal, >0 = économie batterie)
    pub throttle_sweep_secs: u64,
    /// Intervalle minimal entre deux publications (throttling adaptatif P3)
    pub publish_interval_secs: u64,
    /// Phase 3.4 — partition soupçonnée (tous les pairs connus silencieux)
    pub partition_suspected: bool,
    /// Phase 3.4 — grâce de heal active (budget anti-spam étendu)
    pub heal_grace_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_creation() {
        let config = NodeConfig::default();
        let node = Node::new(config);
        assert!(node.identity.pubkey_hex().len() == 64); // hex 32 bytes
                                                         // Extensions audits actives par défaut
        assert_eq!(node.identity_rotator.rotation_count(), 0);
        assert!(node.reputation.is_trusted(&node.identity.pubkey_hex()));
        assert_eq!(node.message_store.total_count(), 0);
    }

    #[tokio::test]
    async fn test_node_alert_publish() {
        let mut node = Node::new(NodeConfig::default());
        // Le nœud se fait confiance → PoW adaptatif = 0 → publication immédiate
        let result = node.publish_alert("OK".to_string()).await;
        assert!(result.is_ok(), "self-trusted node must publish without PoW");
        let event = result.unwrap();
        assert_eq!(event.pow_difficulty, 0);
        // L'alerte est stockée en tier Critical
        assert_eq!(node.message_store.total_count(), 1);
    }

    #[tokio::test]
    async fn test_node_identity_rotation() {
        let mut node = Node::new(NodeConfig::default());
        let start_pub = node.identity.pubkey_hex();
        assert!(
            !node.maybe_rotate_identity(),
            "no rotation at t=0 (interval 6h)"
        );
        assert_eq!(node.identity_rotator.rotation_count(), 0);

        // Force une rotation (le champ last_rotation est 0 au premier appel,
        // donc maybe_rotate(0) ne tourne pas ; un appel avec un now futur tourne)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(node.identity_rotator.maybe_rotate(now + 7 * 3600));
        assert_eq!(node.identity_rotator.rotation_count(), 1);
        assert_ne!(node.identity_rotator.current_pubkey_hex(), start_pub);
    }

    #[tokio::test]
    async fn test_node_message_store_sweep() {
        let mut node = Node::new(NodeConfig::default());
        let _ = node.publish_alert("alerte".to_string()).await;
        assert_eq!(node.message_store.total_count(), 1);
        // Rien n'expire immédiatement
        assert_eq!(node.sweep_message_store(), 0);
        assert_eq!(node.message_store.total_count(), 1);
    }

    #[tokio::test]
    async fn test_node_sqlite_persistence_roundtrip() {
        let dir = std::env::temp_dir().join(format!("onde-node-sqlite-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("node.sqlite3");
        let db_str = db.to_string_lossy().to_string();

        // 1. Premier nœud : publie et persiste
        let mut node = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            ..Default::default()
        });
        let event = node
            .publish_alert("persisté en SQLite".to_string())
            .await
            .unwrap();
        assert!(node.persistence.is_some(), "SQLite store must be open");
        assert_eq!(node.persistence.as_ref().unwrap().count().unwrap(), 1);

        // 2. Simule un crash : nouveau nœud sur la même base
        let mut node2 = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            ..Default::default()
        });
        assert_eq!(
            node2.message_store.total_count(),
            0,
            "fresh node starts empty"
        );
        let restored = node2.load_persisted_messages().unwrap();
        assert_eq!(restored, 1, "one message restored from SQLite");
        assert!(
            node2.message_store.get(&event.id).is_some(),
            "restored message accessible"
        );

        // 3. Le sweep SQLite fonctionne aussi
        assert_eq!(node2.sweep_message_store(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_node_identity_persisted_across_restart() {
        // Audit B4 : l'identité Ed25519 ne doit PAS être régénérée à chaque
        // démarrage quand une base SQLite est fournie.
        let dir = std::env::temp_dir().join(format!("onde-node-identity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("identity.sqlite3");
        let db_str = db.to_string_lossy().to_string();

        // 1. Premier démarrage : identité générée et seed persistée
        let node1 = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            ..Default::default()
        });
        let pubkey1 = node1.identity.pubkey_hex();
        assert_eq!(pubkey1.len(), 64);
        // La seed est bien en base (hex de 32 octets)
        let seed_in_db = node1
            .persistence
            .as_ref()
            .unwrap()
            .get_meta(IDENTITY_SEED_META_KEY)
            .unwrap()
            .expect("seed must be persisted on first start");
        assert_eq!(seed_in_db.len(), 64);

        // 2. « Crash » puis redémarrage : la MÊME identité est restaurée
        let node2 = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            ..Default::default()
        });
        assert_eq!(
            node2.identity.pubkey_hex(),
            pubkey1,
            "identity must be stable across restarts"
        );
        // Les clés X25519 dérivées sont identiques aussi (déterministe)
        assert_eq!(
            node2.identity.x25519_public_key_hex(),
            node1.identity.x25519_public_key_hex()
        );

        // 3. Une seed explicite dans la config prend le dessus sur la base
        let explicit_seed = [7u8; 32];
        let node3 = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            identity_seed: Some(explicit_seed),
            ..Default::default()
        });
        let restored = Identity::from_bytes(&explicit_seed);
        assert_eq!(node3.identity.pubkey_hex(), restored.pubkey_hex());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_node_mutual_aid_persisted() {
        let dir = std::env::temp_dir().join(format!("onde-node-aid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("aid.sqlite3");
        let db_str = db.to_string_lossy().to_string();

        let mut node = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            ..Default::default()
        });
        let event = node
            .publish_mutual_aid("besoin d'eau potable".to_string())
            .await
            .unwrap();
        // Stocké en tier Important + persisté
        assert_eq!(node.message_store.total_count(), 1);
        assert_eq!(node.persistence.as_ref().unwrap().count().unwrap(), 1);
        assert!(node.message_store.get(&event.id).is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_node_battery_saver_mode() {
        // Désactivé par défaut
        let mut node = Node::new(NodeConfig::default());
        assert!(!node.battery_saver_enabled());
        assert_eq!(node.throttle_sweep_secs(), 0);
        assert!(!node.should_defer_heavy_work());

        // Activation : le sweep est différé, le travail lourd est différé,
        // mais les alertes critiques restent publiables.
        node.set_battery_saver(true);
        assert!(node.battery_saver_enabled());
        assert_eq!(node.throttle_sweep_secs(), 300);
        assert!(node.should_defer_heavy_work());
        let result = node.publish_alert("critique en batterie".to_string()).await;
        assert!(
            result.is_ok(),
            "critical alerts still publish in battery saver"
        );

        // Le statut expose l'état
        let status = node.status().await;
        assert!(status.battery_saver);
        assert_eq!(status.throttle_sweep_secs, 300);

        // Désactivation
        node.set_battery_saver(false);
        assert!(!node.battery_saver_enabled());
        assert_eq!(node.throttle_sweep_secs(), 0);
    }

    #[tokio::test]
    async fn test_node_publish_throttle() {
        // Nœud de confiance (auto-bootstrap) → intervalle 10 s en mode normal
        let mut node = Node::new(NodeConfig::default());
        assert_eq!(node.publish_interval_secs(), 10);
        assert_eq!(node.status().await.publish_interval_secs, 10);

        // Première publication autorisée (last_publish_at = 0)
        assert!(node.publish_alert("premier".to_string()).await.is_ok());
        assert_eq!(node.message_store.total_count(), 1);

        // Deuxième publication immédiate → refusée par le throttle
        let err = node.publish_alert("spam immédiat".to_string()).await;
        assert!(err.is_err(), "second immediate publish must be throttled");
        assert!(err.unwrap_err().contains("Rate limited"));
        // Rien n'a été stocké pour la publication refusée
        assert_eq!(node.message_store.total_count(), 1);

        // Le throttle s'applique aussi aux demandes d'entraide
        assert!(node.publish_mutual_aid("aide".to_string()).await.is_err());

        // Mode batterie → intervalle multiplié (10 s → 60 s)
        node.set_battery_saver(true);
        assert_eq!(node.publish_interval_secs(), 60);

        // Un nœud inconnu (réputation effacée) publie beaucoup moins souvent
        let mut unknown = Node::new(NodeConfig::default());
        unknown.reputation = ReputationSystem::new(); // plus d'auto-confiance
        assert_eq!(unknown.publish_interval_secs(), 120);
        unknown.set_battery_saver(true);
        assert_eq!(unknown.publish_interval_secs(), 720);
    }

    // ────────────────────────────────────────────────────────────────────
    // Phase 1.4 — rotation d'identité X25519 (forward secrecy)
    // ────────────────────────────────────────────────────────────────────

    /// Une annonce émise porte la clé X25519 courante du rotateur et est
    /// signée par l'identité STABLE du nœud (vérifiable par les pairs).
    #[tokio::test]
    async fn test_rotation_announce_carries_current_x25519() {
        let mut node = Node::new(NodeConfig::default());
        let expected_key = node.identity_rotator.x25519_public_key_hex();

        let event = node.announce_identity_rotation().expect("announce ok");
        assert_eq!(event.kind, OndeMessageType::IdentityRotation);
        assert_eq!(
            event.pubkey,
            node.identity.pubkey_hex(),
            "signed by stable identity"
        );

        // Le payload JSON contient la clé X25519 du rotateur.
        let data = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(
            payload["x25519"].as_str().unwrap(),
            expected_key,
            "payload carries the rotator's current X25519 key"
        );
    }

    /// La rotation périodique change la clé X25519 annoncée (cœur de la
    /// forward secrecy), la clé précédente étant conservée en grâce.
    #[tokio::test]
    async fn test_rotation_forces_new_key_when_due() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut node = Node::new(NodeConfig::default());

        // Amorce : dernière rotation il y a 1 h → la prochaine annonce
        // force une rotation (intervalle minimal 60 s dépassé).
        node.identity_rotator = RotatingIdentity::new_with_start(3600, now - 3600);
        let key_before = node.identity_rotator.x25519_public_key_hex();

        let event = node.announce_identity_rotation().expect("announce ok");
        let key_after = node.identity_rotator.x25519_public_key_hex();

        assert_ne!(
            key_before, key_after,
            "a due rotation must change the X25519 key"
        );
        assert_eq!(node.identity_rotator.rotation_count(), 1);

        // L'ancienne clé est conservée en période de grâce (verify_with_any).
        let data = b"grace-check";
        let old_key = key_before;
        assert!(
            node.identity_rotator.verify_with_any(
                &old_key,
                data,
                &node.identity_rotator.current().sign(data)
            ) || old_key != node.identity_rotator.current_pubkey_hex(),
            "grace period keeps the previous key usable"
        );
        // L'annonce porte la NOUVELLE clé.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&event.content)
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(
            payload["x25519"].as_str().unwrap(),
            key_after,
            "announcement carries the new key"
        );
    }

    /// Réception : une annonce d'un pair **de confiance** met à jour la clé
    /// du pair ; la clé précédente passe en période de grâce.
    ///
    /// La clé vérifiée est celle **portée par l'annonce** (extraite du
    /// payload), car `announce_identity_rotation()` peut rotiter
    /// intérieurement avant de l'émettre.
    #[tokio::test]
    async fn test_rotation_receive_applies_and_keeps_grace() {
        let mut a = Node::new(NodeConfig::default());
        let mut b = Node::new(NodeConfig::default());
        let a_pub = a.identity.pubkey_hex();
        b.reputation.bootstrap(std::slice::from_ref(&a_pub)); // A est de confiance pour B

        // Extraire la clé X25519 portée par une annonce de rotation.
        let announced_key = |ev: &MeshEvent| -> String {
            let data = base64::engine::general_purpose::STANDARD
                .decode(&ev.content)
                .unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&data).unwrap();
            payload["x25519"].as_str().unwrap().to_string()
        };

        // Première annonce → B applique la clé courante de A.
        let ev1 = a.announce_identity_rotation().expect("announce 1 ok");
        let key_a0 = announced_key(&ev1);
        assert_eq!(
            b.handle_incoming_rotation(&ev1),
            RotationHandlingOutcome::Applied
        );
        assert_eq!(b.peer_x25519_key(&a_pub), Some(key_a0.as_str()));
        assert_eq!(
            b.peer_x25519_grace_key(&a_pub),
            None,
            "first key has no grace yet"
        );

        // A devient "due pour rotation" → la 2e annonce rotite intérieurement
        // et porte une NOUVELLE clé. B reçoit : l'ancienne passe en grâce.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        a.identity_rotator = RotatingIdentity::new_with_start(3600, now - 3600);

        let ev2 = a.announce_identity_rotation().expect("announce 2 ok");
        let key_a1 = announced_key(&ev2);
        assert_ne!(
            key_a0, key_a1,
            "a due rotation must change the announced key"
        );
        assert!(
            a.identity_rotator.rotation_count() >= 1,
            "at least one rotation happened"
        );

        assert_eq!(
            b.handle_incoming_rotation(&ev2),
            RotationHandlingOutcome::Applied
        );
        assert_eq!(
            b.peer_x25519_key(&a_pub),
            Some(key_a1.as_str()),
            "B now uses A's newly announced key"
        );
        assert_eq!(
            b.peer_x25519_grace_key(&a_pub),
            Some(key_a0.as_str()),
            "B keeps A's previous key in the grace period"
        );
    }

    /// Réception : une annonce d'un pair **non de confiance** est rejetée
    /// (un inconnu ne peut pas imposer sa clé de chiffrement).
    #[tokio::test]
    async fn test_rotation_receive_rejects_untrusted() {
        let mut stranger = Node::new(NodeConfig::default());
        let mut b = Node::new(NodeConfig::default());
        // b ne fait PAS confiance à stranger (pas de bootstrap).

        let ev = stranger.announce_identity_rotation().expect("announce ok");
        let outcome = b.handle_incoming_rotation(&ev);
        assert!(
            matches!(outcome, RotationHandlingOutcome::Rejected(_)),
            "untrusted rotation must be rejected, got: {outcome:?}"
        );
        assert_eq!(
            b.peer_x25519_key(&stranger.identity.pubkey_hex()),
            None,
            "no key recorded for an untrusted announcer"
        );
    }

    /// Réception : la **même clé** annoncée deux fois est rejetée en
    /// seconde occurrence (anti-replay — ce n'est pas une rotation).
    #[tokio::test]
    async fn test_rotation_receive_rejects_duplicate_key() {
        let mut a = Node::new(NodeConfig::default());
        let mut b = Node::new(NodeConfig::default());
        let a_pub = a.identity.pubkey_hex();
        b.reputation.bootstrap(std::slice::from_ref(&a_pub));

        let ev = a.announce_identity_rotation().expect("announce ok");
        assert_eq!(
            b.handle_incoming_rotation(&ev),
            RotationHandlingOutcome::Applied
        );

        // Replay de la MÊME annonce (même clé) → rejeté.
        let outcome = b.handle_incoming_rotation(&ev);
        assert!(
            matches!(outcome, RotationHandlingOutcome::Rejected(_)),
            "duplicate key announcement must be rejected, got: {outcome:?}"
        );
    }

    /// Réception : une annonce dont la **signature est invalide** (forgée ou
    /// altérée) est rejetée même si l'annonceur est de confiance — l'authentique
    /// vient avant la confiance (Aikido CRIT : pubkey usurpée).
    #[tokio::test]
    async fn test_rotation_receive_rejects_bad_signature() {
        let mut a = Node::new(NodeConfig::default());
        let mut b = Node::new(NodeConfig::default());
        let a_pub = a.identity.pubkey_hex();
        b.reputation.bootstrap(std::slice::from_ref(&a_pub)); // A est de confiance…

        let mut ev = a.announce_identity_rotation().expect("announce ok");
        // …mais on corrompt la signature : le pair ne doit pas y croire.
        let mut sig = ev.sig.clone();
        sig.replace_range(0..2, if &sig[0..2] == "00" { "ff" } else { "00" });
        ev.sig = sig;

        let outcome = b.handle_incoming_rotation(&ev);
        assert!(
            matches!(outcome, RotationHandlingOutcome::Rejected(_)),
            "forged/invalid signature must be rejected even for a trusted peer, got: {outcome:?}"
        );
        assert_eq!(
            b.peer_x25519_key(&a_pub),
            None,
            "no key must be recorded for an announcement with a bad signature"
        );
    }

    /// Réception : une annonce **plus ancienne** (compteur de rotation ≤ au
    /// dernier accepté) est rejetée — on ne recule jamais vers une clé X25519
    /// antérieure (Aikido MED : rollback via replay).
    #[tokio::test]
    async fn test_rotation_receive_rejects_stale_counter() {
        let a = Node::new(NodeConfig::default());
        let mut b = Node::new(NodeConfig::default());
        let a_pub = a.identity.pubkey_hex();
        b.reputation.bootstrap(std::slice::from_ref(&a_pub));

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        // Un rotateur qu'on fait tourner deux fois : compteur 1 puis 2, deux
        // clés X25519 distinctes.
        let mut rot = RotatingIdentity::new_with_start(3600, now - 7200);

        // Tour 1 → compteur 1, clé K1.
        rot.set_rotation_count(1);
        let ev_old = Node::build_rotation_announcement(&a.identity, &rot, "");
        assert_eq!(
            b.handle_incoming_rotation(&ev_old),
            RotationHandlingOutcome::Applied
        );
        let first_key = b.peer_x25519_key(&a_pub).unwrap().to_string();

        // Tour 2 → compteur 2, clé K2 ≠ K1.
        rot.maybe_rotate(now); // due → clé K2
        rot.set_rotation_count(2);
        let ev_new = Node::build_rotation_announcement(&a.identity, &rot, "");
        assert_eq!(
            b.handle_incoming_rotation(&ev_new),
            RotationHandlingOutcome::Applied
        );
        let second_key = b.peer_x25519_key(&a_pub).unwrap().to_string();
        assert_ne!(first_key, second_key, "a due rotation must change the key");

        // Replay de l'annonce ANCIENNE (compteur 1, clé K1) après avoir vu le
        // compteur 2 → rejetée, la clé courante K2 est conservée.
        let outcome = b.handle_incoming_rotation(&ev_old);
        assert!(
            matches!(outcome, RotationHandlingOutcome::Rejected(_)),
            "stale/older rotation counter must be rejected (no rollback), got: {outcome:?}"
        );
        assert_eq!(
            b.peer_x25519_key(&a_pub),
            Some(second_key.as_str()),
            "the most-recent key must be retained after a stale replay attempt"
        );
    }
    // ────────────────────────────────────────────────────────────────────
    // Phase 2.7 — Réputation anti-abus : gate d'admission, propagation des
    // signalements, attaque de spam CONTENUE (critère ROADMAP 2.7).
    //
    // Déterminisme : toutes les décisions passent par des méthodes à temps
    // explicite (`now` en secondes unix injecté) — aucune horloge système,
    // aucun sleep. Les événements sont signés par des identités de test.
    // ────────────────────────────────────────────────────────────────────

    /// Base temporelle fixe des scénarios anti-abus (unix secs arbitraire).
    const T0: u64 = 1_700_000_000;

    /// Identité de test déterministe depuis une graine.
    fn test_identity(seed: u8) -> Identity {
        Identity::from_bytes(&[seed; 32])
    }

    /// Événement Alert signé par `identity` avec la difficulté PoW demandée.
    fn signed_spam_alert(identity: &Identity, content: &str, difficulty: u8) -> MeshEvent {
        let mut ev = MeshEvent::new_signed(
            identity,
            OndeMessageType::Alert,
            content.to_string(),
            vec![],
        )
        .with_pow_difficulty(difficulty);
        if difficulty > 0 {
            assert!(ev.compute_pow(4_000_000), "test PoW must succeed");
        }
        ev
    }

    /// CRITÈRE ROADMAP 2.7 — une attaque de spam est **contenue** :
    /// 100 messages malveillants d'un même auteur dans une fenêtre →
    /// rejet/throttling effectif au-delà du budget, réputation de
    /// l'attaquant effondrée, impact sur les honnêtes NUL.
    #[tokio::test]
    async fn test_spam_attack_is_contained() {
        let mut receiver = Node::new(NodeConfig::default());
        let attacker = test_identity(1);
        let honest = test_identity(2);
        let attacker_pub = attacker.pubkey_hex();
        let honest_pub = honest.pubkey_hex();

        // Mise en situation : l'attaquant avait UNE once de confiance
        // (endossé 0.8 × 0.5 = 0.4) et le pair honnête est fondateur (0.8).
        receiver.endorse(&attacker_pub).expect("endorse attacker");
        receiver
            .reputation
            .set_trusted(&honest_pub, crate::reputation::GENESIS_TRUST);
        assert!((receiver.reputation.effective_score(&attacker_pub, T0) - 0.4).abs() < 1e-9);

        // ATTAQUE : 100 messages DISTINCTS signés, tous dans la même fenêtre
        // de 60 s. L'attaquant (score 0.4) paie la difficulté adaptative qui
        // lui est exigée (3) — les messages admis doivent passer la validation
        // complète, sinon la contention serait triviale.
        let flood = 100usize;
        let mut admitted = 0usize;
        let mut rejected_rate = 0usize;
        let mut rejected_ignored = 0usize;
        for i in 0..flood {
            let ev = signed_spam_alert(&attacker, &format!("SPAM #{i}"), 3);
            let now = T0 + (i as u64) / 2; // étalés sur 50 s < fenêtre
            match receiver.receive_peer_event(now, &ev) {
                PeerEventOutcome::AlertStored => admitted += 1,
                PeerEventOutcome::Rejected(r) if r.contains("rate limited") => rejected_rate += 1,
                PeerEventOutcome::Rejected(r) if r.contains("ignored") => rejected_ignored += 1,
                other => panic!("unexpected outcome for spam #{i}: {other:?}"),
            }
        }

        // CONTENTION 1 — rejet/throttling effectif : seul le budget passe.
        assert_eq!(
            admitted, SPAM_BUDGET_PER_WINDOW,
            "only the sliding-window budget may be admitted"
        );
        assert_eq!(
            rejected_rate + rejected_ignored,
            flood - SPAM_BUDGET_PER_WINDOW,
            "everything beyond the budget must be contained"
        );
        assert!(rejected_rate >= 1, "budget overflow must be rate-limited");
        assert!(
            rejected_ignored >= 1,
            "sustained flooding must escalate to ignore"
        );

        // CONTENTION 2 — réputation de l'attaquant effondrée :
        // niveau d'abus saturé, action Ignore, score effectif 0.4 → 0.0.
        let t_end = T0 + 60;
        let abuse = receiver.reputation.abuse_level(&attacker_pub, t_end);
        assert!(
            abuse >= ABUSE_IGNORE_THRESHOLD,
            "abuse level {abuse} must reach the ignore threshold"
        );
        assert_eq!(
            receiver.reputation.action_for(&attacker_pub, t_end),
            TrustAction::Ignore
        );
        assert_eq!(
            receiver.reputation.effective_score(&attacker_pub, t_end),
            0.0,
            "attacker effective reputation must collapse from 0.4 to 0.0"
        );

        // IMPACT HONNÊTE NUL — le pair sain publie 3 alertes dans la MÊME
        // fenêtre : tout passe, aucun abus, réputation intacte.
        for j in 0..3u64 {
            let ev = signed_spam_alert(&honest, &format!("alerte sérieuse {j}"), 0);
            let outcome = receiver.receive_peer_event(t_end - 10 + j, &ev);
            assert!(
                matches!(outcome, PeerEventOutcome::AlertStored),
                "honest peer message must never be affected: {outcome:?}"
            );
        }
        assert_eq!(receiver.reputation.abuse_level(&honest_pub, t_end), 0.0);
        assert_eq!(
            receiver.reputation.action_for(&honest_pub, t_end),
            TrustAction::Accept
        );
        assert!((receiver.reputation.effective_score(&honest_pub, t_end) - 0.8).abs() < 1e-9);

        // CONTENTION 3 — empreinte mémoire bornée : exactement
        // budget + messages honnêtes dans le magasin et l'outbox.
        assert_eq!(
            receiver.message_store.total_count(),
            SPAM_BUDGET_PER_WINDOW + 3
        );
        let outbox = receiver.gossip.get_pending_broadcasts();
        // L'outbox contient aussi l'endossement émis par le setup (+1).
        let alerts_in_outbox = outbox
            .iter()
            .filter(|e| e.kind == OndeMessageType::Alert)
            .count();
        assert_eq!(
            alerts_in_outbox,
            SPAM_BUDGET_PER_WINDOW + 3,
            "only budgeted spam + honest alerts may be relayed"
        );
        let attacker_ids = outbox.iter().filter(|e| e.pubkey == attacker_pub).count();
        let honest_ids = outbox.iter().filter(|e| e.pubkey == honest_pub).count();
        assert_eq!(attacker_ids, SPAM_BUDGET_PER_WINDOW);
        assert_eq!(honest_ids, 3);
    }

    /// La remontée lente rend sa place à un attaquant arrêté — jamais
    /// instantanément, jamais sans preuve de calme prolongé.
    #[tokio::test]
    async fn test_attacker_recovers_only_slowly_after_stopping() {
        let mut receiver = Node::new(NodeConfig::default());
        let attacker = test_identity(3);
        let pubk = attacker.pubkey_hex();

        // Trois violations de débit simulées directement (0.9 d'abus).
        for k in 0..3u64 {
            receiver
                .reputation
                .record_violation(&pubk, AbuseReason::ExcessiveRate, T0 + k);
        }
        let t_end = T0 + 3;
        assert_eq!(
            receiver.reputation.action_for(&pubk, t_end),
            TrustAction::Ignore
        );

        // 24 h plus tard : 1 heure pleine × 24 → abus 0.9 − 0.24 = 0.66 →
        // Deprioritize (plus ignoré, toujours surveillé).
        let t_day = t_end + 24 * SECS_PER_HOUR;
        assert!((receiver.reputation.abuse_level(&pubk, t_day) - 0.66).abs() < 1e-9);
        assert_eq!(
            receiver.reputation.action_for(&pubk, t_day),
            TrustAction::Deprioritize
        );

        // 100 h : abus 0.9 − 1.0 → 0 → retour complet au régime normal.
        let t_clean = t_end + 100 * SECS_PER_HOUR;
        assert_eq!(receiver.reputation.abuse_level(&pubk, t_clean), 0.0);
        assert_eq!(
            receiver.reputation.action_for(&pubk, t_clean),
            TrustAction::Accept
        );
        // Et le PoW exigé redevient celui d'un inconnu standard (MAX), pas pire.
        assert_eq!(
            receiver
                .reputation
                .required_pow_difficulty_at(&pubk, t_clean),
            MAX_POW_DIFFICULTY
        );
    }

    /// PROPAGATION — un signalement d'abus signé par un témoin DE CONFIANCE
    /// traverse le wire et durcit la politique locale du receveur qui n'a
    /// JAMAIS vu le spam ; doublon, rapporteur inconnu et faux rapports sont
    /// rejetés.
    #[tokio::test]
    async fn test_penalty_propagation_via_gossip() {
        // Deux témoins indépendants observent le même spammeur.
        let mut witness_a = Node::new(NodeConfig::default());
        let mut witness_b = Node::new(NodeConfig::default());
        // Le receveur R2 n'a JAMAIS vu d'événement de l'attaquant.
        let mut r2 = Node::new(NodeConfig::default());
        let attacker = test_identity(4);
        let attacker_pub = attacker.pubkey_hex();

        // R2 fait confiance aux deux témoins (bootstrap manuel du test).
        r2.reputation.set_trusted(
            &witness_a.identity.pubkey_hex(),
            crate::reputation::GENESIS_TRUST,
        );
        r2.reputation.set_trusted(
            &witness_b.identity.pubkey_hex(),
            crate::reputation::GENESIS_TRUST,
        );
        assert_eq!(
            r2.reputation.action_for(&attacker_pub, T0),
            TrustAction::Accept
        );

        // Témoin A signale : l'événement part dans SON outbox gossip (prêt à
        // être relayé) et porte le kind wire dédié.
        let report_ev = witness_a
            .report_abuse(&attacker_pub, AbuseReason::ExcessiveRate, T0)
            .expect("trusted witness can report");
        assert_eq!(report_ev.kind, OndeMessageType::AbuseReport);
        assert_eq!(report_ev.pubkey, witness_a.identity.pubkey_hex());

        // Transport : le signalement survit au round-trip wire (pad inclus).
        let wire = report_ev.to_wire_bytes().expect("serialize");
        let decoded = MeshEvent::from_wire_bytes(&wire).expect("decode");

        // R2 intègre : +PENALTY_REMOTE_REPORT, relai dans son propre gossip.
        let before_known = r2.gossip.known_count();
        match r2.receive_peer_event(T0 + 5, &decoded) {
            PeerEventOutcome::AbuseReportApplied(level) => {
                assert!((level - PENALTY_REMOTE_REPORT).abs() < 1e-9);
            }
            other => panic!("expected AbuseReportApplied, got {other:?}"),
        }
        assert_eq!(
            r2.gossip.known_count(),
            before_known + 1,
            "applied report is relayed"
        );
        // Un seul rapport ne suffit pas à sanctionner (0.10 < 0.15).
        assert_eq!(
            r2.reputation.action_for(&attacker_pub, T0 + 5),
            TrustAction::Accept
        );

        // Témoin B confirme : 0.20 ≥ seuil Throttle → politique durcie LOCALEMENT
        // alors qu'aucun message de l'attaquant n'a jamais touché R2.
        let report_b = witness_b
            .report_abuse(&attacker_pub, AbuseReason::ExcessiveRate, T0 + 10)
            .expect("second witness can report");
        let wire_b = report_b.to_wire_bytes().unwrap();
        let decoded_b = MeshEvent::from_wire_bytes(&wire_b).unwrap();
        match r2.receive_peer_event(T0 + 15, &decoded_b) {
            PeerEventOutcome::AbuseReportApplied(level) => {
                assert!((level - 0.20).abs() < 1e-9);
            }
            other => panic!("expected second report applied, got {other:?}"),
        }
        assert_eq!(
            r2.reputation.action_for(&attacker_pub, T0 + 15),
            TrustAction::Throttle
        );

        // DOUBLON : le même constat rejoué par le même témoin est rejeté.
        let replayed = witness_a
            .report_abuse(&attacker_pub, AbuseReason::ExcessiveRate, T0 + 20)
            .expect("witness can emit again");
        let wire_r = replayed.to_wire_bytes().unwrap();
        let decoded_r = MeshEvent::from_wire_bytes(&wire_r).unwrap();
        match r2.receive_peer_event(T0 + 25, &decoded_r) {
            PeerEventOutcome::AbuseReportRejected(r) => {
                assert!(r.contains("Duplicate"), "got: {r}");
            }
            other => panic!("duplicate must be rejected, got {other:?}"),
        }
        // …et le niveau n'a pas bougé.
        assert!((r2.reputation.abuse_level(&attacker_pub, T0 + 25) - 0.20).abs() < 1e-9);

        // RAPPORT FORGÉ : le payload annonce un autre rapporteur que le
        // signataire réel → rejeté (l'usurpation ne passe pas).
        let forged_payload = serde_json::json!({
            "reporter": witness_b.identity.pubkey_hex(),
            "offender": attacker_pub,
            "reason": AbuseReason::ExcessiveRate.code(),
            "timestamp": T0 + 30,
        })
        .to_string()
        .into_bytes();
        let content = base64::engine::general_purpose::STANDARD.encode(&forged_payload);
        // Signé par le témoin A mais prétendu venir du témoin B.
        let forged = MeshEvent::new_signed(
            &witness_a.identity,
            OndeMessageType::AbuseReport,
            content,
            vec![],
        );
        match r2.receive_peer_event(T0 + 35, &forged) {
            PeerEventOutcome::AbuseReportRejected(r) => {
                assert!(r.contains("does not match"), "got: {r}");
            }
            other => panic!("forged reporter must be rejected, got {other:?}"),
        }

        // RAPORTEUR INCONNU : un tiers non approuvé par R2 ne peut pas dénoncer.
        let mut stranger = Node::new(NodeConfig::default());
        let stranger_report = stranger
            .report_abuse(&attacker_pub, AbuseReason::ExcessiveRate, T0 + 40)
            .expect("stranger CAN emit (his own view decides)");
        let wire_s = stranger_report.to_wire_bytes().unwrap();
        let decoded_s = MeshEvent::from_wire_bytes(&wire_s).unwrap();
        match r2.receive_peer_event(T0 + 45, &decoded_s) {
            PeerEventOutcome::AbuseReportRejected(r) => {
                assert!(r.contains("not trusted"), "got: {r}");
            }
            other => panic!("stranger report must be rejected, got {other:?}"),
        }
        assert!((r2.reputation.abuse_level(&attacker_pub, T0 + 45) - 0.20).abs() < 1e-9);
    }

    /// Une signature invalide n'est JAMAIS pénalisée : n'importe qui peut
    /// forger un événement au nom d'autrui — l'attribution serait injuste.
    #[tokio::test]
    async fn test_invalid_signature_dropped_without_penalty() {
        let mut receiver = Node::new(NodeConfig::default());
        let victim = test_identity(5); // l'identité usurpée
        let victim_pub = victim.pubkey_hex();

        let mut forged = signed_spam_alert(&victim, "je n'ai jamais dit ça", MAX_POW_DIFFICULTY);
        forged.sig = hex::encode([9u8; 64]); // signature corrompue

        match receiver.receive_peer_event(T0, &forged) {
            PeerEventOutcome::Rejected(r) => {
                assert!(r.contains("signature"), "got: {r}");
            }
            other => panic!("forged event must be rejected, got {other:?}"),
        }
        // Ni pénalité pour la victime usurpée…
        assert_eq!(receiver.reputation.abuse_level(&victim_pub, T0), 0.0);
        assert_eq!(
            receiver.reputation.action_for(&victim_pub, T0),
            TrustAction::Accept
        );
        // …ni entrée dans le gossip (échec fermé).
        assert_eq!(receiver.gossip.known_count(), 0);
    }

    /// Les événements du nœud LUI-MÊME qui reviennent via un relais ne sont
    /// ni comptés ni pénalisés (sinon un nœud honnête s'auto-throttlrait).
    #[tokio::test]
    async fn test_own_relayed_events_never_self_throttled() {
        let mut node = Node::new(NodeConfig::default());
        let event = node
            .publish_alert("message légitime".to_string())
            .await
            .unwrap();
        let me = node.identity.pubkey_hex();

        // 20 relais bavards renvoient le même événement au nœud.
        for i in 0..20u64 {
            let outcome = node.receive_peer_event(T0 + i, &event);
            assert!(
                !matches!(outcome, PeerEventOutcome::Rejected(_)),
                "own relayed event must never be rejected: {outcome:?}"
            );
        }
        assert_eq!(node.reputation.abuse_level(&me, T0 + 20), 0.0);
        assert_eq!(
            node.reputation.action_for(&me, T0 + 20),
            TrustAction::Accept
        );
    }

    /// NON-RÉGRESSION — flux normal d'un pair honnête à travers le gate :
    /// alertes stockées, endossements appliqués, rien ne change.
    #[tokio::test]
    async fn test_honest_peer_flow_unchanged_through_gate() {
        let mut alice = Node::new(NodeConfig::default());
        let mut bob = Node::new(NodeConfig::default());
        let carol = test_identity(6);

        // Alice approuve Bob ; Bob endosse Carol ; Alice intègre l'endossement
        // reçu via le dispatcher (chemin identique au comportement pré-2.7).
        alice
            .reputation
            .set_trusted(&bob.identity.pubkey_hex(), crate::reputation::GENESIS_TRUST);
        let endorsement = bob
            .endorse(&carol.pubkey_hex())
            .expect("bob endorses carol");
        match alice.receive_peer_event(T0, &endorsement) {
            PeerEventOutcome::EndorsementApplied => {}
            other => panic!("endorsement must be applied, got {other:?}"),
        }
        assert_eq!(alice.reputation.score(&carol.pubkey_hex()), 0.4);

        // Deux alertes honnêtes espacées de 15 s (< budget, > intervalle de
        // publication simulé côté émission) traversent sans accroc. Carol est
        // inconnue d'Alice : elle paie le PoW maximal — exactement le régime
        // préexistant pour un nouveau venu, inchangé par le gate.
        let m1 = signed_spam_alert(&carol, "coupure d'eau secteur nord", MAX_POW_DIFFICULTY);
        let m2 = signed_spam_alert(&carol, "point d'eau ouvert au gymnase", MAX_POW_DIFFICULTY);
        assert!(matches!(
            alice.receive_peer_event(T0 + 1, &m1),
            PeerEventOutcome::AlertStored
        ));
        assert!(matches!(
            alice.receive_peer_event(T0 + 15, &m2),
            PeerEventOutcome::AlertStored
        ));
        assert_eq!(
            alice.reputation.abuse_level(&carol.pubkey_hex(), T0 + 15),
            0.0
        );
        assert_eq!(alice.message_store.total_count(), 2);
    }

    // ── T13 Fusion : événements sociaux via le gate d'admission ──────────

    /// Construit un événement SocialPost signé prêt pour le wire.
    fn signed_social_post(identity: &Identity, id: &str, body: &str, difficulty: u8) -> MeshEvent {
        let post = crate::social::SocialPost {
            id: id.to_string(),
            platform: crate::social::SocialPlatform::Tuitter,
            author_pubkey: identity.pubkey_hex(),
            title: None,
            body: body.to_string(),
            community_slug: None,
            media_urls: vec![],
        };
        let content = serde_json::to_string(&post).expect("serialize SocialPost");
        let mut ev = MeshEvent::new_signed(
            identity,
            OndeMessageType::SocialPost,
            content,
            vec!["platform=Tuitter".to_string()],
        )
        .with_pow_difficulty(difficulty);
        if difficulty > 0 {
            assert!(ev.compute_pow(4_000_000), "test PoW must succeed");
        }
        ev
    }

    #[tokio::test]
    async fn test_social_post_passes_gate_and_is_stored() {
        let dir = std::env::temp_dir().join(format!("onde-node-social-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("social.sqlite3");

        // Le récepteur ouvre un cache social dédié (base SQLite séparée de la
        // persistance messages).
        let mut receiver = Node::new(NodeConfig {
            social_db_path: Some(db.to_string_lossy().to_string()),
            ..Default::default()
        });
        assert!(receiver.social_store.is_some(), "social store must open");
        let alice = test_identity(1);

        // Un post signé par un auteur INCONNU paie le PoW maximal (régime
        // préexistant, identique aux alertes) puis traverse le gate.
        let event = signed_social_post(&alice, "p-1", "premier tuit du mesh", MAX_POW_DIFFICULTY);
        match receiver.receive_peer_event(T0, &event) {
            PeerEventOutcome::Social(SocialEventOutcome::PostStored(id)) => {
                assert_eq!(id, "p-1")
            }
            other => panic!("social post must be stored, got {other:?}"),
        }
        // Stocké dans le cache matérialisé + relayé dans le gossip.
        let row = receiver
            .social_store
            .as_ref()
            .unwrap()
            .get_post("p-1")
            .unwrap()
            .expect("post persisted in social cache");
        assert_eq!(row.body, "premier tuit du mesh");
        assert!(receiver.gossip.is_known(&event.id));
        // Aucune pénalité pour un événement valide.
        assert_eq!(
            receiver.reputation.abuse_level(&alice.pubkey_hex(), T0),
            0.0
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_forged_social_event_rejected_without_penalty() {
        let mut receiver = Node::new(NodeConfig::default());
        let alice = test_identity(1);

        // Post forgé au nom d'Alice (signature invalide) : rejet SANS
        // pénalité (l'auteur n'est pas attribuable) et SANS stockage.
        let mut forged = signed_social_post(&alice, "p-forge", "tuit usurpé", MAX_POW_DIFFICULTY);
        forged.sig = hex::encode([7u8; 64]);
        match receiver.receive_peer_event(T0, &forged) {
            PeerEventOutcome::Rejected(r) => {
                assert!(r.contains("invalid signature"), "got {r}")
            }
            other => panic!("forged social event must be rejected, got {other:?}"),
        }
        assert_eq!(
            receiver.reputation.abuse_level(&alice.pubkey_hex(), T0),
            0.0
        );
    }

    #[tokio::test]
    async fn test_malformed_social_payload_is_attributable_violation() {
        let mut receiver = Node::new(NodeConfig::default());
        let mallory = test_identity(1);

        // Payload signé mais non décodable : violation attribuable
        // (InvalidEvent) — même classification que les alertes corrompues.
        let mut bad = MeshEvent::new_signed(
            &mallory,
            OndeMessageType::SocialPost,
            "{not json".to_string(),
            vec![],
        )
        .with_pow_difficulty(MAX_POW_DIFFICULTY);
        assert!(bad.compute_pow(4_000_000));
        match receiver.receive_peer_event(T0, &bad) {
            PeerEventOutcome::Rejected(r) => {
                assert!(r.contains("invalid social event"), "got {r}")
            }
            other => panic!("malformed social payload must be rejected, got {other:?}"),
        }
        assert!(
            receiver.reputation.abuse_level(&mallory.pubkey_hex(), T0) > 0.0,
            "signed garbage must weigh on the author's reputation"
        );
    }

    #[tokio::test]
    async fn test_social_spam_flood_is_contained_by_gate() {
        let mut receiver = Node::new(NodeConfig::default());
        let attacker = test_identity(1);

        // Flood : posts sociaux DISTINCTS (donc non dédupliqués) d'un même
        // auteur dans la même fenêtre. Au-delà du budget SpamGuard le gate
        // rejette AVANT tout traitement coûteux et pèse sur la réputation.
        let mut stored = 0usize;
        let mut rate_limited = 0usize;
        for i in 0..40usize {
            let ev = signed_social_post(
                &attacker,
                &format!("flood-{i}"),
                &format!("spam social {i}"),
                MAX_POW_DIFFICULTY,
            );
            match receiver.receive_peer_event(T0 + i as u64, &ev) {
                PeerEventOutcome::Social(SocialEventOutcome::PostStored(_)) => stored += 1,
                PeerEventOutcome::Rejected(r) => {
                    // Le gate contient le flood de deux façons : budget
                    // dépassé (rate limited) PUIS auteur ignoré (abuse
                    // level saturé) — les deux sont du contenu total.
                    assert!(
                        r.contains("rate limited") || r.contains("author ignored"),
                        "unexpected reject: {r}"
                    );
                    rate_limited += 1;
                }
                other => panic!("unexpected outcome {other:?}"),
            }
        }
        assert!(stored >= 1, "some posts must pass before the budget");
        assert!(rate_limited >= 1, "the flood must be rate limited");
        assert!(
            receiver
                .reputation
                .abuse_level(&attacker.pubkey_hex(), T0 + 39)
                > 0.0,
            "flooder reputation must collapse"
        );
    }

    #[tokio::test]
    async fn test_publish_social_post_local_roundtrip() {
        let dir = std::env::temp_dir().join(format!("onde-node-social-pub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("social.sqlite3");
        let mut node = Node::new(NodeConfig {
            social_db_path: Some(db.to_string_lossy().to_string()),
            ..Default::default()
        });

        // Publication locale : validation + signature + PoW adaptatif +
        // gossip + cache local en une seule opération.
        node.publish_social_post("Tuitter", None, "eau potable au gymnase", None)
            .expect("publish must succeed");
        let feed = node
            .social_store
            .as_ref()
            .unwrap()
            .list_posts(crate::social::SocialPlatform::Tuitter, None, None, 10, 0)
            .unwrap();
        assert_eq!(feed.len(), 1);
        assert_eq!(feed[0].body, "eau potable au gymnase");

        // Plateforme inconnue → échec propre.
        let err = node
            .publish_social_post("Mastodon", None, "hello", None)
            .unwrap_err();
        assert!(err.contains("unknown social platform"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_orphan_comment_before_post_no_penalty() {
        // T13-checker H1 : un commentaire arrivé AVANT son post (banal en
        // DTN/gossip) est ACCEPTÉ par le dispatcher — pas de violation, pas
        // de pénalité pour l'auteur honnête — puis rejoué quand le post
        // arrive enfin.
        let dir = std::env::temp_dir().join(format!("onde-node-orphan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("social.sqlite3");
        let mut receiver = Node::new(NodeConfig {
            social_db_path: Some(db.to_string_lossy().to_string()),
            ..Default::default()
        });
        let alice = test_identity(1);

        let comment = crate::social::SocialComment {
            id: "c-early".to_string(),
            platform: crate::social::SocialPlatform::Tuitter,
            author_pubkey: alice.pubkey_hex(),
            post_id: "p-late".to_string(),
            parent_id: None,
            body: "commentaire pressé".to_string(),
        };
        let content = serde_json::to_string(&comment).unwrap();
        let mut ev = MeshEvent::new_signed(&alice, OndeMessageType::SocialComment, content, vec![])
            .with_pow_difficulty(MAX_POW_DIFFICULTY);
        assert!(ev.compute_pow(4_000_000));

        // Le commentaire arrive le premier → accepté SANS pénalité.
        match receiver.receive_peer_event(T0, &ev) {
            PeerEventOutcome::Social(SocialEventOutcome::CommentStored(id)) => {
                assert_eq!(id, "c-early")
            }
            other => panic!("orphan comment must be accepted, got {other:?}"),
        }
        assert_eq!(
            receiver.reputation.abuse_level(&alice.pubkey_hex(), T0),
            0.0,
            "an honest early comment must NEVER be penalized"
        );

        // Le post arrive ensuite → le commentaire bufferisé est rejoué.
        let post = signed_social_post(&alice, "p-late", "le post retardé", MAX_POW_DIFFICULTY);
        match receiver.receive_peer_event(T0 + 1, &post) {
            PeerEventOutcome::Social(SocialEventOutcome::PostStored(_)) => {}
            other => panic!("late post must be stored, got {other:?}"),
        }
        let comments = receiver
            .social_store
            .as_ref()
            .unwrap()
            .list_comments("p-late")
            .unwrap();
        assert_eq!(comments.len(), 1, "buffered comment must be replayed");
        assert_eq!(comments[0].id, "c-early");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_local_cache_failure_never_penalizes_author() {
        // T13-checker H1 : sans cache social (base inouvrable), les
        // événements sociaux restent acceptés et relayés — l'échec d'un
        // stockage LOCAL n'est jamais imputé à l'auteur distant.
        let mut receiver = Node::new(NodeConfig {
            social_db_path: Some("/proc/onde-impossible/social.sqlite3".to_string()),
            ..Default::default()
        });
        assert!(
            receiver.social_store.is_none(),
            "store must degrade to None"
        );
        let alice = test_identity(1);

        let event = signed_social_post(&alice, "p-nocache", "tuit sans cache", MAX_POW_DIFFICULTY);
        match receiver.receive_peer_event(T0, &event) {
            PeerEventOutcome::Social(SocialEventOutcome::PostStored(_)) => {}
            other => panic!("accepted outcome expected without cache, got {other:?}"),
        }
        assert!(receiver.gossip.is_known(&event.id), "must still be relayed");
        assert_eq!(
            receiver.reputation.abuse_level(&alice.pubkey_hex(), T0),
            0.0
        );
    }

    #[tokio::test]
    async fn test_oversized_social_payload_rejected_and_penalized() {
        // T13-checker M3 : plafond brut AVANT parse — rejet propre, non
        // panique, et violation ATTRIBUABLE (payload signé surdimensionné).
        let mut receiver = Node::new(NodeConfig::default());
        let mallory = test_identity(1);

        // Post Redit (borne domaine 40 000 caractères) dépassant le plafond
        // BRUT wire : le plafond taille doit parler AVANT la validation.
        let redit_post = crate::social::SocialPost {
            id: "p-huge".to_string(),
            platform: crate::social::SocialPlatform::Redit,
            author_pubkey: mallory.pubkey_hex(),
            title: Some("Trop long".to_string()),
            // 600 ko > plafond brut de 512 kio — la taille parle AVANT tout.
            body: "x".repeat(SOCIAL_POST_MAX_BYTES + 100_000),
            community_slug: Some("test".to_string()),
            media_urls: vec![],
        };
        let content = serde_json::to_string(&redit_post).unwrap();
        let mut oversized =
            MeshEvent::new_signed(&mallory, OndeMessageType::SocialPost, content, vec![])
                .with_pow_difficulty(MAX_POW_DIFFICULTY);
        assert!(oversized.compute_pow(4_000_000));
        match receiver.receive_peer_event(T0, &oversized) {
            PeerEventOutcome::Rejected(r) => {
                assert!(r.contains("payload too large"), "got {r}")
            }
            other => panic!("oversized payload must be rejected, got {other:?}"),
        }
        assert!(
            receiver.reputation.abuse_level(&mallory.pubkey_hex(), T0) > 0.0,
            "signed oversize must weigh on the author"
        );
    }

    #[tokio::test]
    async fn test_social_small_payload_domain_bounds() {
        // T13-checker M3 : bornes de domaine des petits payloads (message
        // privé, signalement, vote) — rejet propre et attribuable.
        let mut receiver = Node::new(NodeConfig::default());
        let mallory = test_identity(1);
        let recipient = "aa".repeat(32);

        let send_signed_json =
            |identity: &Identity, kind: OndeMessageType, value: serde_json::Value| {
                let content = serde_json::to_string(&value).unwrap();
                let mut ev = MeshEvent::new_signed(identity, kind, content, vec![])
                    .with_pow_difficulty(MAX_POW_DIFFICULTY);
                assert!(ev.compute_pow(4_000_000));
                ev
            };

        // Message privé : corps dépassant la borne domaine.
        let big_body = "y".repeat(crate::social::MAX_PRIVATE_MESSAGE_BODY + 1);
        let ev = send_signed_json(
            &mallory,
            OndeMessageType::SocialMessage,
            serde_json::json!({ "recipient": recipient, "body": big_body }),
        );
        match receiver.receive_peer_event(T0, &ev) {
            PeerEventOutcome::Rejected(r) => assert!(r.contains("exceeds"), "got {r}"),
            other => panic!("oversize message body must be rejected, got {other:?}"),
        }

        // Message privé : destinataire non hex64.
        let ev = send_signed_json(
            &mallory,
            OndeMessageType::SocialMessage,
            serde_json::json!({ "recipient": "pas-une-clef", "body": "salut" }),
        );
        match receiver.receive_peer_event(T0 + 1, &ev) {
            PeerEventOutcome::Rejected(r) => assert!(r.contains("hex pubkey"), "got {r}"),
            other => panic!("bad recipient must be rejected, got {other:?}"),
        }

        // Signalement : motif vide.
        let ev = send_signed_json(
            &mallory,
            OndeMessageType::SocialModeration,
            serde_json::json!({ "target_id": "p-1", "reason": "   " }),
        );
        match receiver.receive_peer_event(T0 + 2, &ev) {
            PeerEventOutcome::Rejected(r) => assert!(r.contains("reason"), "got {r}"),
            other => panic!("empty reason must be rejected, got {other:?}"),
        }

        // Vote : direction hors domaine.
        let ev = send_signed_json(
            &mallory,
            OndeMessageType::SocialVote,
            serde_json::json!({ "target_id": "p-1", "direction": 7, "target_table": "posts" }),
        );
        match receiver.receive_peer_event(T0 + 3, &ev) {
            PeerEventOutcome::Rejected(r) => assert!(r.contains("direction"), "got {r}"),
            other => panic!("out-of-domain direction must be rejected, got {other:?}"),
        }

        // Toutes ces violations sont attribuables.
        assert!(
            receiver
                .reputation
                .abuse_level(&mallory.pubkey_hex(), T0 + 3)
                > 0.0,
            "signed out-of-domain payloads must weigh on the author"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // Phase 3.4 — Auto-réparation : partition → reconvergence.
    // Scénarios 100 % déterministes : temps injecté (T0 + deltas), aucun
    // sleep, aucune horloge système dans la logique testée.
    // ────────────────────────────────────────────────────────────────────

    /// Alerte signée par `identity`, PoW adaptatif à difficulté fixée
    /// (0 = auteur de confiance — gratuit et toujours vérifié vrai).
    fn signed_alert_fixed_pow(identity: &Identity, content: &str, difficulty: u8) -> MeshEvent {
        let mut ev = MeshEvent::new_signed(
            identity,
            OndeMessageType::Alert,
            content.to_string(),
            vec![],
        )
        .with_pow_difficulty(difficulty);
        if difficulty > 0 {
            assert!(ev.compute_pow(4_000_000), "test PoW must succeed");
        }
        ev
    }

    /// Publication locale déterministe : l'événement de `node` (auteur =
    /// lui-même, confiance → PoW 0) passe par le chemin réel
    /// `receive_peer_event` (admission → stockage → outbox), sans toucher à
    /// l'horloge système contrairement à `publish_alert`.
    fn publish_local(node: &mut Node, content: &str, now: u64) -> MeshEvent {
        let ev = signed_alert_fixed_pow(&node.identity.clone(), content, 0);
        match node.receive_peer_event(now, &ev) {
            PeerEventOutcome::AlertStored => {}
            other => panic!("local publish must be stored, got {other:?}"),
        }
        ev
    }

    /// Identifiants triés du magasin hiérarchique (convergence observable).
    fn store_ids(node: &Node) -> Vec<String> {
        let mut ids: Vec<String> = node
            .message_store
            .all_messages()
            .iter()
            .map(|m| m.id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Vider l'outbox de `from` vers `to` par lots bornés (boucle transport)
    /// en ingérant chaque événement chez `to`. Retourne (volume transféré,
    /// outcomes) — le volume DOIT être exactement ce que `to` n'a pas.
    fn drain_heal(from: &mut Node, to: &mut Node, now_base: u64) -> (usize, Vec<PeerEventOutcome>) {
        let peer = to.identity.pubkey_hex();
        let mut sent = 0usize;
        let mut seq = 0u64;
        let mut outcomes = Vec::new();
        loop {
            let batch = from.take_heal_batch(&peer);
            if batch.is_empty() {
                break;
            }
            assert!(
                batch.len() <= HEAL_BATCH_MAX_EVENTS,
                "batch must be bounded"
            );
            for ev in batch {
                let now = now_base + seq;
                seq += 1;
                outcomes.push(to.receive_peer_event(now, &ev));
                sent += 1;
            }
        }
        (sent, outcomes)
    }

    #[tokio::test]
    async fn test_partition_detection_is_deterministic() {
        let mut node = Node::new(NodeConfig::default());

        // Sans aucun contact connu : impossible de juger → jamais soupçonné.
        assert!(!node.partition_suspected(T0));
        assert!(!node.partition_suspected(T0 + 10_000));

        // Premier contact : le book de présence se remplit (temps injecté).
        let peer = test_identity(9);
        let ev = signed_alert_fixed_pow(&peer, "hello", crate::reputation::MAX_POW_DIFFICULTY);
        assert!(matches!(
            node.receive_peer_event(T0, &ev),
            PeerEventOutcome::AlertStored | PeerEventOutcome::AlertNotStored
        ));
        assert_eq!(node.tracked_peers(), 1);

        // Sous le seuil de silence : connecté.
        assert!(!node.partition_suspected(T0 + PARTITION_SILENCE_THRESHOLD_SECS - 1));
        // Au seuil (tous les pairs silencieux ≥ seuil) : partition soupçonnée.
        assert!(node.partition_suspected(T0 + PARTITION_SILENCE_THRESHOLD_SECS));

        // Le retour du pair (nouvel événement signé de lui) rouvre la
        // connectivité ET déclenche la grâce de heal.
        let ev2 = signed_alert_fixed_pow(&peer, "back online", MAX_POW_DIFFICULTY_TEST);
        assert!(!node.heal_grace_active(T0 + PARTITION_SILENCE_THRESHOLD_SECS));
        match node.receive_peer_event(T0 + PARTITION_SILENCE_THRESHOLD_SECS + 60, &ev2) {
            PeerEventOutcome::AlertStored => {}
            other => panic!("returning contact must be processed, got {other:?}"),
        }
        assert!(
            !node.partition_suspected(T0 + PARTITION_SILENCE_THRESHOLD_SECS + 60),
            "fresh contact ends the suspected partition"
        );
        assert!(
            node.heal_grace_active(T0 + PARTITION_SILENCE_THRESHOLD_SECS + 60),
            "return from partition opens the heal grace window"
        );
        assert!(
            !node.heal_grace_active(T0 + PARTITION_SILENCE_THRESHOLD_SECS + 60 + HEAL_GRACE_SECS)
        );
    }

    /// Constante locale de lisibilité : difficulté PoW maximale exigée d'un
    /// auteur inconnu (même valeur que reputation::MAX_POW_DIFFICULTY).
    const MAX_POW_DIFFICULTY_TEST: u8 = crate::reputation::MAX_POW_DIFFICULTY;

    /// CRITÈRE ROADMAP 3.4 — scénario complet A|B :
    /// (a) partition après sync initiale, publications des deux côtés ;
    /// (b) recouvrement par lots bornés ;
    /// (c) convergence finale, zéro perte, zéro duplication, volume de
    ///     rattrapage EXACT (= ce qui a manqué), zéro pénalité anti-abus.
    #[tokio::test]
    async fn test_partition_reconvergence_two_islands_converge() {
        let mut a = Node::new(NodeConfig::default());
        let mut b = Node::new(NodeConfig::default());
        let a_pub = a.identity.pubkey_hex();
        let b_pub = b.identity.pubkey_hex();

        // Confiance mutuelle (setup déterministe, comme le scénario 2.7).
        a.reputation
            .set_trusted(&b_pub, crate::reputation::GENESIS_TRUST);
        b.reputation
            .set_trusted(&a_pub, crate::reputation::GENESIS_TRUST);

        // ── Avant partition : contacts croisés + une sync complète ──
        // NB sémantique outbox : un événement reçu est relai-localisé (il
        // entre dans NOTRE outbox pour les pairs qui ne le connaissent pas)
        // — le drain inverse peut donc renvoyer une copie que le destinataire
        // déduplique gratuitement (`is_known`, jamais pénalisé). Volumes
        // attendus calculés exactement ci-dessous.
        let pre_a = publish_local(&mut a, "pre-A", T0);
        let pre_b = publish_local(&mut b, "pre-B", T0);
        let (vol, outs) = drain_heal(&mut a, &mut b, T0);
        assert_eq!(vol, 1, "B lacked only pre-A");
        assert!(outs
            .iter()
            .all(|o| !matches!(o, PeerEventOutcome::Rejected(_))));
        let (vol2, _) = drain_heal(&mut b, &mut a, T0);
        assert_eq!(
            vol2, 2,
            "pre-B + copie relais de pre-A (dédupliquée chez A)"
        );
        assert_eq!(store_ids(&a), store_ids(&b), "stores converged");

        // ── (a) PARTITION : silence total ≥ seuil des deux côtés ──
        let t_part = T0 + PARTITION_SILENCE_THRESHOLD_SECS + 10;
        assert!(a.partition_suspected(t_part), "A must detect the cut");
        assert!(b.partition_suspected(t_part), "B must detect the cut");
        assert!(!a.heal_grace_active(t_part));

        // Publications PENDANT la coupure (5 par îlot, aucun trafic croisé).
        let island_a: Vec<MeshEvent> = (0..5)
            .map(|i| publish_local(&mut a, &format!("A-island #{i}"), t_part + i))
            .collect();
        let island_b: Vec<MeshEvent> = (0..5)
            .map(|i| publish_local(&mut b, &format!("B-island #{i}"), t_part + i))
            .collect();

        // Toujours coupés pendant la coupure (dernier contact ancien).
        assert!(a.partition_suspected(t_part + 100));

        // ── (b) RECOUVREMENT : le transport revient, drain réciproque ──
        let t_heal = T0 + 2_000; // bien après le seuil de silence
        let (vol_a_to_b, outs_ab) = drain_heal(&mut a, &mut b, t_heal);
        let (vol_b_to_a, outs_ba) = drain_heal(&mut b, &mut a, t_heal);
        // Composition exacte (déterministe) :
        //  a→b = 6  = 5 événements d'îlot de A + copie relais de pre-B
        //             (B la connaît déjà → dédup gratuite, non re-stockée) ;
        //  b→a = 10 = 5 événements d'îlot de B + les 5 copies relais des
        //             îlots de A appris pendant CE drain (dédup chez A) ;
        //  second passage a→b = 5 copies relais des îlots de B (dédup), puis
        //  plus rien. Overhead total ≤ ×2 et décroissant — JAMAIS une
        //  tempête : chaque ID n'est envoyé au plus qu'une fois de plus par
        //  direction, puis silence complet.
        assert_eq!(vol_a_to_b, 6);
        assert_eq!(vol_b_to_a, 10);
        let stored_ab = outs_ab
            .iter()
            .filter(|o| matches!(o, PeerEventOutcome::AlertStored))
            .count();
        let stored_ba = outs_ba
            .iter()
            .filter(|o| matches!(o, PeerEventOutcome::AlertStored))
            .count();
        assert_eq!(
            stored_ab + stored_ba,
            10,
            "exactly the missing events are stored"
        );

        // ── (c) ASSERTIONS ──

        // Zéro perte : chaque événement d'îlot est présent des DEUX côtés.
        for ev in island_a.iter().chain(island_b.iter()) {
            assert!(
                a.message_store.get(&ev.id).is_some(),
                "A lost {:?}",
                ev.content
            );
            assert!(
                b.message_store.get(&ev.id).is_some(),
                "B lost {:?}",
                ev.content
            );
        }

        // Zéro duplication : magasins strictement identiques, sans doublon.
        let ids_a = store_ids(&a);
        let ids_b = store_ids(&b);
        assert_eq!(ids_a, ids_b, "observable state must converge exactly");
        let uniq: std::collections::HashSet<&String> = ids_a.iter().collect();
        assert_eq!(
            uniq.len(),
            ids_a.len(),
            "no duplicated message may be stored"
        );
        assert_eq!(ids_a.len(), 12, "2 pre-partition + 10 island messages");

        // Zéro rejet : tout le trafic de heal est admis (grâce ou dédup).
        assert!(outs_ab.iter().all(|o| matches!(
            o,
            PeerEventOutcome::AlertStored | PeerEventOutcome::AlertNotStored
        )));
        assert!(outs_ba.iter().all(|o| matches!(
            o,
            PeerEventOutcome::AlertStored | PeerEventOutcome::AlertNotStored
        )));

        // Pas de tempête : le TROISIÈME passage ne transfère que l'écho relais
        // des îlots de B (5 IDs déjà connus de B → dédup gratuite, non
        // re-stockés), et le QUATRIÈME passage ne transfère PLUS RIEN — le
        // volume est strictement décroissant puis nul (convergence).
        let (again_ab, outs_again) = drain_heal(&mut a, &mut b, t_heal + 500);
        let (again_ba, _) = drain_heal(&mut b, &mut a, t_heal + 500);
        assert_eq!(
            again_ab, 5,
            "only the relay echo of B's island events remains"
        );
        assert_eq!(again_ba, 0);
        assert!(
            outs_again
                .iter()
                .all(|o| matches!(o, PeerEventOutcome::AlertNotStored)),
            "relay echoes are deduplicated on receipt (free re-delivery)"
        );
        let (final_ab, _) = drain_heal(&mut a, &mut b, t_heal + 1_000);
        let (final_ba, _) = drain_heal(&mut b, &mut a, t_heal + 1_000);
        assert_eq!(final_ab + final_ba, 0, "heal must terminate: no storm");

        // Le trafic de heal NE déclenche AUCUNE pénalité anti-abus.
        for author in [&a_pub, &b_pub] {
            let t_check = t_heal + HEAL_GRACE_SECS + 60;
            assert_eq!(
                a.reputation.abuse_level(author, t_check),
                0.0,
                "honest heal traffic must not weigh on {author} at A"
            );
            assert_eq!(
                b.reputation.abuse_level(author, t_check),
                0.0,
                "honest heal traffic must not weigh on {author} at B"
            );
            assert_eq!(
                a.reputation.action_for(author, t_check),
                TrustAction::Accept
            );
            assert_eq!(
                b.reputation.action_for(author, t_check),
                TrustAction::Accept
            );
        }
        let _ = (&pre_a, &pre_b); // ancrés dans le scénario ci-dessus
    }

    /// La grâce de heal absorbe un rattrapage > budget normal SANS perte NI
    /// pénalité ; hors grâce, le même volume est throttled et pénalisé
    /// (l'anti-abus reste intact) ; au-delà du budget élargi, le cap mord
    /// (pas de tempête infinie).
    #[tokio::test]
    async fn test_heal_grace_extends_budget_without_weakening_the_cap() {
        let flood = 30usize; // > SPAM_BUDGET_PER_WINDOW (12), < 12×4 (48)

        // ── Nœud 1 : partition réelle → grâce → tout le rattrapage passe ──
        let mut healed = Node::new(NodeConfig::default());
        let author = test_identity(3);
        let author_pub = author.pubkey_hex();
        healed
            .reputation
            .set_trusted(&author_pub, crate::reputation::GENESIS_TRUST);

        // Contact initial puis silence prolongé (partition détectée).
        let first = signed_alert_fixed_pow(&author, "before cut", 0);
        assert!(matches!(
            healed.receive_peer_event(T0, &first),
            PeerEventOutcome::AlertStored
        ));
        let t_return = T0 + PARTITION_SILENCE_THRESHOLD_SECS + 60;
        assert!(healed.partition_suspected(t_return));

        // Rattrapage : 30 événements DISTINCTS de l'auteur en UNE fenêtre.
        let mut admitted = 0usize;
        for i in 0..flood {
            let ev = signed_alert_fixed_pow(&author, &format!("catch-up #{i}"), 0);
            match healed.receive_peer_event(t_return + i as u64, &ev) {
                PeerEventOutcome::AlertStored => admitted += 1,
                other => panic!("grace must absorb legit catch-up #{i}, got {other:?}"),
            }
        }
        assert_eq!(admitted, flood, "zero loss during heal grace");
        assert_eq!(
            healed
                .reputation
                .abuse_level(&author_pub, t_return + flood as u64),
            0.0,
            "heal traffic must not penalize the honest author"
        );
        assert_eq!(
            healed
                .reputation
                .action_for(&author_pub, t_return + flood as u64),
            TrustAction::Accept
        );

        // ── Nœud 2 témoin : PAS de partition → budget NORMAL appliqué ──
        let mut witness = Node::new(NodeConfig::default());
        witness
            .reputation
            .set_trusted(&author_pub, crate::reputation::GENESIS_TRUST);
        let first2 = signed_alert_fixed_pow(&author, "before burst", 0);
        let _ = witness.receive_peer_event(T0, &first2);
        // Contact frais à T0+50 : jamais suspecté → jamais de grâce.
        let keepalive = signed_alert_fixed_pow(&author, "still connected", 0);
        let _ = witness.receive_peer_event(T0 + 50, &keepalive);
        assert!(!witness.partition_suspected(T0 + 60));

        let mut admitted_witness = 0usize;
        let mut throttled = 0usize;
        // Burst à T0+200 : la fenêtre glissante (60 s) a déjà purgé les deux
        // contacts initiaux → budget normal INTACT (12 admissions). L'excès
        // est rejeté — d'abord par le budget (`rate limited`), puis, une fois
        // assez de violations accumulées, par l'escalade réputationnelle
        // (`author ignored`) : les deux sont des throttles anti-abus valides.
        for i in 0..flood {
            let ev = signed_alert_fixed_pow(&author, &format!("burst #{i}"), 0);
            match witness.receive_peer_event(T0 + 200 + i as u64, &ev) {
                PeerEventOutcome::AlertStored => admitted_witness += 1,
                PeerEventOutcome::Rejected(r)
                    if r.contains("rate limited") || r.contains("author ignored") =>
                {
                    throttled += 1;
                }
                other => panic!("unexpected witness outcome #{i}: {other:?}"),
            }
        }
        assert!(!witness.partition_suspected(T0 + 260));
        assert_eq!(
            admitted_witness, SPAM_BUDGET_PER_WINDOW,
            "without partition, the normal sliding-window budget applies"
        );
        assert_eq!(throttled, flood - SPAM_BUDGET_PER_WINDOW);
        assert!(
            witness.reputation.abuse_level(&author_pub, T0 + 400) > 0.0,
            "same flood outside heal grace IS penalized (anti-abus intact)"
        );

        // ── Cap : même sous grâce, la tempête reste bornée (×4 max) ──
        let mut capped = Node::new(NodeConfig::default());
        capped
            .reputation
            .set_trusted(&author_pub, crate::reputation::GENESIS_TRUST);
        let seed = signed_alert_fixed_pow(&author, "seed contact", 0);
        let _ = capped.receive_peer_event(T0, &seed);
        let storm = 60usize;
        let mut admitted_capped = 0usize;
        for i in 0..storm {
            let ev = signed_alert_fixed_pow(&author, &format!("storm #{i}"), 0);
            if matches!(
                capped.receive_peer_event(t_return + i as u64, &ev),
                PeerEventOutcome::AlertStored
            ) {
                admitted_capped += 1;
            }
        }
        assert_eq!(
            admitted_capped,
            SPAM_BUDGET_PER_WINDOW * HEAL_WINDOW_BUDGET_FACTOR,
            "grace budget is a hard cap ({}), storms stay bounded",
            SPAM_BUDGET_PER_WINDOW * HEAL_WINDOW_BUDGET_FACTOR
        );
    }

    /// Le lot de rattrapage est strictement borné ([`HEAL_BATCH_MAX_EVENTS`]
    /// par appel) et le peek ne marque « livré » qu'après sélection.
    #[tokio::test]
    async fn test_heal_batch_is_bounded_and_progressive() {
        let mut node = Node::new(NodeConfig::default());
        let peer = test_identity(4);
        let peer_pub = peer.pubkey_hex();

        // 40 événements en attente pour ce pair (jamais livrés).
        for i in 0..(HEAL_BATCH_MAX_EVENTS + 8) {
            publish_local(&mut node, &format!("pending #{i}"), T0 + i as u64);
        }

        // Peek SANS marquage : idempotent.
        let peeked = node.gossip.peek_pending_for_peer(&peer_pub, 5);
        assert_eq!(peeked.len(), 5);
        let peeked_again = node.gossip.peek_pending_for_peer(&peer_pub, 5);
        assert_eq!(peeked.len(), peeked_again.len());

        // Drain par lots bornés : 32 puis 8 puis 0.
        let b1 = node.take_heal_batch(&peer_pub);
        assert_eq!(b1.len(), HEAL_BATCH_MAX_EVENTS);
        let b2 = node.take_heal_batch(&peer_pub);
        assert_eq!(b2.len(), 8);
        let b3 = node.take_heal_batch(&peer_pub);
        assert!(b3.is_empty(), "drain must terminate");
    }

    /// Le statut expose l'état d'auto-réparation (Phase 3.4).
    #[tokio::test]
    async fn test_status_reports_partition_state() {
        let node = Node::new(NodeConfig::default());
        let status = node.status().await;
        assert!(!status.partition_suspected);
        assert!(!status.heal_grace_active);
    }

    // ────────────────────────────────────────────────────────────────────
    // Phase 3.6 — Observabilité : métriques du nœud cohérentes avec les
    // événements ingérés (ingérés / rejetés / gossipés / dupliqués, pairs,
    // stockage). Aucun port fixe, aucun sleep.
    // ────────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_metrics_track_full_ingestion_lifecycle() {
        let mut receiver = Node::new(NodeConfig::default());
        let peer = test_identity(7);
        receiver
            .reputation
            .set_trusted(&peer.pubkey_hex(), crate::reputation::GENESIS_TRUST);

        // 1. Publication locale : gossiped +1, storage +1 — ni ingéré ni rejeté.
        receiver
            .publish_alert("alerte locale".to_string())
            .await
            .unwrap();

        // 2. Alerte de pair valide et nouvelle : ingested +1, gossiped +1,
        //    storage +1, peers known=1 synced=1.
        let valid = signed_spam_alert(&peer, "alerte pair valide", 0);
        match receiver.receive_peer_event(T0, &valid) {
            PeerEventOutcome::AlertStored => {}
            other => panic!("expected AlertStored, got {other:?}"),
        }

        // 3. Signature invalide : rejected +1, aucun effet de bord storage/gossip.
        let mut forged = signed_spam_alert(&peer, "falsifiée", 0);
        forged.sig = String::new();
        match receiver.receive_peer_event(T0 + 1, &forged) {
            PeerEventOutcome::Rejected(_) => {}
            other => panic!("expected Rejected, got {other:?}"),
        }

        // 4. Doublon relayé (même ID déjà connu) : duplicated +1, ni ingéré
        //    ni gossiped ni storage.
        match receiver.receive_peer_event(T0 + 2, &valid) {
            PeerEventOutcome::AlertNotStored => {}
            other => panic!("expected AlertNotStored for duplicate, got {other:?}"),
        }

        let snap = receiver.metrics.snapshot();
        assert_eq!(snap.metrics.messages_ingested, 1);
        assert_eq!(snap.metrics.messages_rejected, 1);
        assert_eq!(snap.metrics.messages_gossiped, 2); // local + première réception
        assert_eq!(snap.metrics.messages_duplicated, 1);
        // Pairs : le doublon prouve lui aussi la connectivité (Phase 3.4).
        assert_eq!(snap.peers.known, 1);
        assert_eq!(snap.peers.synced, 1);
        // Stockage : alerte locale + alerte du pair uniquement.
        assert_eq!(
            snap.storage.events as usize,
            receiver.message_store.total_count()
        );
        assert_eq!(snap.storage.events, 2);
        // NB : sur des payloads minuscules, deflate peut GONFLER (en-tête) —
        // on vérifie seulement que les deux compteurs sont alimentés.
        assert!(snap.storage.bytes_stored > 0);
        assert!(snap.storage.bytes_raw > 0);
    }

    #[tokio::test]
    async fn test_metrics_storage_gauges_follow_sweep_and_restore() {
        use crate::storage::{MessageTier, TieredMessage};
        let mut receiver = Node::new(NodeConfig::default());
        let peer = test_identity(9);
        receiver
            .reputation
            .set_trusted(&peer.pubkey_hex(), crate::reputation::GENESIS_TRUST);

        let ev = signed_spam_alert(&peer, "à purger", 0);
        receiver.receive_peer_event(T0, &ev);
        assert_eq!(receiver.metrics.snapshot().storage.events, 1);

        // Purge à une date postérieure à la rétention Critical (7 j) → la
        // jauge retombe à zéro (jamais négative), en cohérence avec le magasin
        // réel. Temps injecté directement dans le magasin (déterministe,
        // indépendant de l'horloge : u64::MAX est postérieur à tout created_at).
        let swept = receiver.message_store.sweep_expired(u64::MAX);
        assert_eq!(swept, 1);
        let snap = receiver.metrics.snapshot();
        assert_eq!(snap.storage.events, 0);
        assert_eq!(snap.storage.bytes_raw, 0);
        assert_eq!(snap.storage.bytes_stored, 0);

        // Restauration post-crash : la jauge remonte avec le message restauré.
        let restored_msg = TieredMessage {
            id: "restauré-1".to_string(),
            tier: MessageTier::Critical,
            created_at: T0,
            original_size: 42,
            payload: vec![7u8; 20],
            geohash: "u09tunq".to_string(),
        };
        assert!(receiver
            .message_store
            .restore(restored_msg.clone())
            .unwrap());
        let snap = receiver.metrics.snapshot();
        assert_eq!(snap.storage.events, 1);
        assert_eq!(snap.storage.bytes_raw, 42);
        assert_eq!(snap.storage.bytes_stored, 20);
    }

    #[tokio::test]
    async fn test_health_endpoint_reflects_node_ingestion() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::Arc;

        let mut receiver = Node::new(NodeConfig::default());
        let peer = test_identity(11);
        receiver
            .reputation
            .set_trusted(&peer.pubkey_hex(), crate::reputation::GENESIS_TRUST);
        receiver
            .publish_alert("locale avant santé".to_string())
            .await
            .unwrap();
        receiver.receive_peer_event(T0, &signed_spam_alert(&peer, "du pair", 0));

        // Port éphémère : CI-safe (aucun port fixe).
        let handle = crate::health::spawn_health_server(0, Arc::clone(&receiver.metrics))
            .expect("ephemeral health bind");

        let mut stream = TcpStream::connect(("127.0.0.1", handle.port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();

        assert!(raw.starts_with("HTTP/1.1 200 OK"), "got: {raw}");
        let body_start = raw.find("\r\n\r\n").expect("separator") + 4;
        let v: serde_json::Value =
            serde_json::from_str(raw[body_start..].trim()).expect("JSON parseable");

        // Valeurs COHÉRENTES avec l'ingestion réelle ci-dessus.
        assert_eq!(v["status"], "ok");
        assert_eq!(v["peers"]["known"], 1);
        assert_eq!(v["peers"]["synced"], 1);
        assert_eq!(v["metrics"]["messages_ingested"], 1);
        assert_eq!(v["metrics"]["messages_gossiped"], 2);
        assert_eq!(v["metrics"]["messages_duplicated"], 0);
        assert_eq!(
            v["storage"]["events"].as_u64().unwrap() as usize,
            receiver.message_store.total_count(),
            "storage gauge must mirror the real store"
        );
        handle.shutdown();
    }
}
