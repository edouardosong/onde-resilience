/// Node Management — Core ONDE node with all subsystems
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::crypto::{Identity, RotatingIdentity, ZkTransaction, TxPool};
use crate::network::YggdrasilAddress;
use crate::protocol::{MeshEvent, OndeMessageType, GossipProtocol};
use crate::reputation::ReputationSystem;
use crate::ai::AiEngine;
use crate::storage::{
    ZimReader, MBTilesRenderer, IpfsSeeder, TieredMessageStore, TieredMessage, StoragePolicy,
    MessageTier, persistence::SqliteStore,
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

    /// Le mode économie batterie est-il actif ?
    pub fn battery_saver_enabled(&self) -> bool {
        self.config.battery_saver
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
}