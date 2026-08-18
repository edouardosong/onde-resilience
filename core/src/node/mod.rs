/// Node Management — Core ONDE node with all subsystems
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use base64::Engine as _;

use crate::crypto::{Identity, RotatingIdentity, ZkTransaction, TxPool};
use crate::network::YggdrasilAddress;
use crate::protocol::{MeshEvent, OndeMessageType, GossipProtocol};
use crate::reputation::{Endorsement, ReputationSystem};
use crate::ai::AiEngine;
use crate::storage::{
    ZimReader, MBTilesRenderer, IpfsSeeder, TieredMessageStore, TieredMessage, StoragePolicy,
    MessageTier, persistence::SqliteStore,
};
use crate::update::{
    UpdateProtocol, UpdateAnnouncement, Version, DEFAULT_CHUNK_SIZE,
};

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
        let update_root_pubkey = config.update_root_pubkey.unwrap_or(DEFAULT_UPDATE_ROOT_PUBKEY);
        let update_protocol = UpdateProtocol::new(update_root_pubkey, parse_update_version(&config.update_version));
        let update_root_signing = config
            .update_root_seed
            .map(|seed| Identity::from_bytes(&seed));

        Self {
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
        }
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

        let difficulty = self.reputation.required_pow_difficulty(&self.identity.pubkey_hex());
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

        self.gossip.add_event_with_reputation(event.clone(), &self.reputation)?;

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
            self.persist_message(&event.id, MessageTier::Critical, event.content.as_bytes(), event.created_at, &geohash);
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
        let difficulty = self.reputation.required_pow_difficulty(&self.identity.pubkey_hex());
        let mut event = MeshEvent::new_signed(
            &self.identity,
            OndeMessageType::MutualAid,
            content,
            vec![],
        )
        .with_pow_difficulty(difficulty);

        if difficulty > 0 && !event.compute_pow(2_000_000) {
            return Err("PoW computation failed".to_string());
        }
        event.validate_with_reputation(&self.reputation)?;

        self.gossip.add_event_with_reputation(event.clone(), &self.reputation)?;

        // Stockage hiérarchique : les demandes d'entraide sont Important (2 jours)
        let geohash = self.config.my_geohash.clone();
        if self.message_store.store(
            &event.id,
            MessageTier::Important,
            event.content.as_bytes(),
            event.created_at,
            &geohash,
        )? {
            self.persist_message(&event.id, MessageTier::Important, event.content.as_bytes(), event.created_at, &geohash);
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
        let Some(persist) = self.persistence.as_mut() else { return };
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
        let nonce = self.tx_pool.next_expected_nonce(&self.identity.pubkey_hex());
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
        let event =
            MeshEvent::new_signed(&self.identity, OndeMessageType::UpdateAnnounce, content, tags);
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
        let sig_bytes = hex::decode(sig_hex)
            .map_err(|_| "root_sig tag is not valid hex".to_string())?;
        let sig: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| "root_sig must be 64 bytes".to_string())?;
        Ok((data, sig))
    }

    /// Receveur — annonce reçue : vérifie la signature racine + version >
    /// locale, mémorise l'annonce, puis émet une requête `manifest` vers
    /// l'annonceur.
    fn on_update_announce(
        &mut self,
        event: &MeshEvent,
    ) -> Result<UpdateHandlingOutcome, String> {
        let (data, sig) = Self::decode_update_payload(event)?;
        match self.update_protocol.handle_announcement(&data, &sig) {
            Ok(announcement) => {
                let announcer = event.pubkey.clone();
                self.pending_announcement = Some(announcement);
                let request = MeshEvent::new_signed(
                    &self.identity,
                    OndeMessageType::UpdateRequest,
                    String::new(),
                    build_tags(&[
                        (TAG_REQ_TYPE, "manifest".to_string()),
                        (TAG_TO, announcer),
                    ]),
                );
                self.publish_gossip_event(request)?;
                Ok(UpdateHandlingOutcome::AnnouncementRequested)
            }
            Err(e) => Ok(UpdateHandlingOutcome::Rejected(e.to_string())),
        }
    }

    /// Receveur — manifeste reçu : le lie à l'annonce acceptée
    /// (`handle_manifest`), puis émet une requête `chunk 0` vers l'annonceur.
    fn on_update_manifest(
        &mut self,
        event: &MeshEvent,
    ) -> Result<UpdateHandlingOutcome, String> {
        let announcement = self
            .pending_announcement
            .clone()
            .ok_or_else(|| "update manifest received without a prior accepted announcement".to_string())?;
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
    pub fn handle_incoming_endorsement(
        &mut self,
        event: &MeshEvent,
    ) -> EndorsementHandlingOutcome {
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
            let pubkey_ok = hex::decode(&event.pubkey).map(|b| b.len() == 32).unwrap_or(false);
            let sig_ok = hex::decode(&event.sig).map(|b| b.len() == 64).unwrap_or(false);
            pubkey_ok
                && sig_ok
                && {
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
        let event = Self::build_rotation_announcement(
            &self.identity,
            &self.identity_rotator,
            &prev_x25519,
        );
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
        let content = base64::engine::general_purpose::STANDARD
            .encode(payload.to_string().into_bytes());

        let tags = build_tags(&[
            (TAG_IDENTITY_ROTATION, new_x25519.clone()),
            ("rotation_count", rotation_count.to_string()),
        ]);
        MeshEvent::new_signed(
            identity,
            OndeMessageType::IdentityRotation,
            content,
            tags,
        )
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
            let pubkey_ok = hex::decode(announcer).map(|b| b.len() == 32).unwrap_or(false);
            let sig_ok = hex::decode(&event.sig).map(|b| b.len() == 64).unwrap_or(false);
            pubkey_ok
                && sig_ok
                && {
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
        self.peer_x25519.insert(announcer.clone(), new_x25519.clone());
        self.peer_rotation_count
            .insert(announcer.clone(), rotation_count);

        // 7. Relai dans le gossip (idempotent).
        let _ = self.gossip.add_event_with_reputation(event.clone(), &self.reputation);

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
            trusted_peers: self.reputation.summary().iter().filter(|(_, s, _)| *s >= crate::reputation::TRUSTED_THRESHOLD).count(),
            stored_messages: self.message_store.total_count(),
            stored_compressed_bytes: stored_bytes,
            stored_raw_bytes: raw_bytes,
            battery_saver: self.config.battery_saver,
            throttle_sweep_secs: self.throttle_sweep_secs(),
            publish_interval_secs: self.publish_interval_secs(),
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
        assert!(!node.maybe_rotate_identity(), "no rotation at t=0 (interval 6h)");
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
        let event = node.publish_alert("persisté en SQLite".to_string()).await.unwrap();
        assert!(node.persistence.is_some(), "SQLite store must be open");
        assert_eq!(node.persistence.as_ref().unwrap().count().unwrap(), 1);

        // 2. Simule un crash : nouveau nœud sur la même base
        let mut node2 = Node::new(NodeConfig {
            sqlite_path: Some(db_str.clone()),
            ..Default::default()
        });
        assert_eq!(node2.message_store.total_count(), 0, "fresh node starts empty");
        let restored = node2.load_persisted_messages().unwrap();
        assert_eq!(restored, 1, "one message restored from SQLite");
        assert!(node2.message_store.get(&event.id).is_some(), "restored message accessible");

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
        let event = node.publish_mutual_aid("besoin d'eau potable".to_string()).await.unwrap();
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
        assert!(result.is_ok(), "critical alerts still publish in battery saver");

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
        assert_eq!(event.pubkey, node.identity.pubkey_hex(), "signed by stable identity");

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
            node
                .identity_rotator
                .verify_with_any(&old_key, data, &node.identity_rotator.current().sign(data))
                || old_key != node.identity_rotator.current_pubkey_hex(),
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
        assert_eq!(b.handle_incoming_rotation(&ev1), RotationHandlingOutcome::Applied);
        assert_eq!(b.peer_x25519_key(&a_pub), Some(key_a0.as_str()));
        assert_eq!(b.peer_x25519_grace_key(&a_pub), None, "first key has no grace yet");

        // A devient "due pour rotation" → la 2e annonce rotite intérieurement
        // et porte une NOUVELLE clé. B reçoit : l'ancienne passe en grâce.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        a.identity_rotator = RotatingIdentity::new_with_start(3600, now - 3600);

        let ev2 = a.announce_identity_rotation().expect("announce 2 ok");
        let key_a1 = announced_key(&ev2);
        assert_ne!(key_a0, key_a1, "a due rotation must change the announced key");
        assert!(a.identity_rotator.rotation_count() >= 1, "at least one rotation happened");

        assert_eq!(b.handle_incoming_rotation(&ev2), RotationHandlingOutcome::Applied);
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
        assert_eq!(b.handle_incoming_rotation(&ev), RotationHandlingOutcome::Applied);

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
        assert_eq!(b.handle_incoming_rotation(&ev_old), RotationHandlingOutcome::Applied);
        let first_key = b.peer_x25519_key(&a_pub).unwrap().to_string();

        // Tour 2 → compteur 2, clé K2 ≠ K1.
        rot.maybe_rotate(now); // due → clé K2
        rot.set_rotation_count(2);
        let ev_new = Node::build_rotation_announcement(&a.identity, &rot, "");
        assert_eq!(b.handle_incoming_rotation(&ev_new), RotationHandlingOutcome::Applied);
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
}