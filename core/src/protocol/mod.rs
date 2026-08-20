/// Protocol layer — Nostr events, PoW antispam, message formats
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{Identity, TrafficPadding};

/// Maximum alert message size (characters)
pub const MAX_ALERT_SIZE: usize = 280;

/// Maximum voice memo duration (seconds)
pub const MAX_VOICE_DURATION: u32 = 120;

/// Maximum acceptable clock skew for event timestamps (5 minutes)
pub const MAX_CLOCK_SKEW_SECS: u64 = 300;

/// Current UNIX timestamp in seconds.
///
/// ⚠️ Fallback sûr (Audit m6) : si l'horloge système est antérieure à
/// l'époque UNIX (horloge non réglée), renvoie 0 plutôt que de paniquer.
/// Un timestamp 0 sera rejeté comme "trop vieux" par les validateurs.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/*
 * Nostr-style Event System
 */

/// ONDE message type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OndeMessageType {
    /// Public alert (280 chars max, no images)
    Alert,
    /// Mutual aid request (hierarchical)
    MutualAid,
    /// Async voice memo (Opus 8kbps)
    VoiceMemo,
    /// Voice-to-text transcription
    Transcription,
    /// ZK transaction
    Transaction,
    /// AI query
    AiQuery,
    /// AI response
    AiResponse,
    /// P2P file share request
    FileShareRequest,
    /// Heartbeat / status
    Heartbeat,
    /// Mise à jour APK — annonce signée (racine) d'une nouvelle version
    /// (Phase 1.1 : câblage du protocole update dans le gossip).
    ///
    /// `content` porte le blob signé (base64) de l'annonce ; `tags` portent
    /// `root_sig`, `version`, `peer`.
    UpdateAnnounce,
    /// Mise à jour APK — manifeste canonique signé (racine) + métadonnées de
    /// transfert non signées. `content` = base64 du message wire complet.
    UpdateManifest,
    /// Mise à jour APK — chunk d'APK (non signé, validé par le hash signé à
    /// l'assemblage). `content` = base64 des octets du chunk ; `tags` portent
    /// `index`, `total`, `peer`.
    UpdateChunk,
    /// Mise à jour APK — demande de manifeste ou de chunk vers l'annonceur.
    /// `content` vide ; `tags` portent `req_type` (manifest|chunk), `to`,
    /// `index` (chunks).
    UpdateRequest,
    /// Web of Trust — endossement signé (Phase 1.2 : propagation des
    /// endossements dans le gossip).
    ///
    /// `content` porte l'`Endorsement` (`endorser`, `endorsed`, `timestamp`)
    /// sérialisé en JSON puis base64 ; `sig` est la signature Ed25519 de
    /// l'endosseur (`pubkey` = `endorser`) sur l'ID canonique ; le PoW
    /// adaptatif s'applique (un endosseur de confiance → difficulté 0).
    Endorsement,
    /// Rotation d'identité (Phase 1.4) — annonce signée d'une nouvelle clé
    /// X25519 de chiffrement point-à-point.
    ///
    /// L'identité **stable** de signature ne change pas (la réputation reste
    /// indexée dessus) ; seule la clé X25519 de chiffrement tourne (forward
    /// secrecy des sessions E2E). `content` porte le JSON `{ "stable_pubkey":
    /// hex, "x25519": hex, "timestamp": u64 }` (base64) ; `sig` est la
    /// signature Ed25519 de l'auteur stable sur l'ID canonique.
    IdentityRotation,
}

/// Nostr-style event for the mesh network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshEvent {
    /// SHA256 hash of the canonical event serialization (event ID)
    pub id: String,
    /// Public key of creator (hex)
    pub pubkey: String,
    /// Unix timestamp
    pub created_at: u64,
    /// Event kind
    pub kind: OndeMessageType,
    /// Tags for routing/filtering
    pub tags: Vec<String>,
    /// Content (text or base64 data)
    pub content: String,
    /// Ed25519 signature over the canonical ID (hex)
    pub sig: String,
    /// Proof-of-Work nonce
    pub pow_nonce: u64,
    /// PoW difficulty target (number of leading zeros)
    pub pow_difficulty: u8,
    /// TTL in hops
    pub ttl: u8,
}

impl MeshEvent {
    /// Create an UNSIGNED event (sig is empty → rejected by `validate`).
    /// Prefer `new_signed` for events that must pass validation.
    pub fn new(pubkey: &str, kind: OndeMessageType, content: String, tags: Vec<String>) -> Self {
        // Horodatage flou ±30 s (Audit #14) : brouille le moment exact
        // d'émission aux observateurs du mesh.
        let created_at = crate::crypto::fuzzy_timestamp_secs();
        let id = Self::compute_id(pubkey, created_at, &kind, &tags, &content);
        Self {
            id,
            pubkey: pubkey.to_string(),
            created_at,
            kind,
            tags,
            content,
            sig: String::new(), // unsigned → rejected by validate()
            pow_nonce: 0,
            pow_difficulty: 4,
            ttl: 5,
        }
    }

    /// Create an event signed with the sender's Ed25519 identity.
    /// The canonical ID is computed first, then signed.
    pub fn new_signed(
        sender: &Identity,
        kind: OndeMessageType,
        content: String,
        tags: Vec<String>,
    ) -> Self {
        let pubkey = sender.pubkey_hex();
        // Horodatage flou ±30 s (Audit #14)
        let created_at = crate::crypto::fuzzy_timestamp_secs();
        let id = Self::compute_id(&pubkey, created_at, &kind, &tags, &content);
        let sig = hex::encode(sender.sign(id.as_bytes()));
        Self {
            id,
            pubkey,
            created_at,
            kind,
            tags,
            content,
            sig,
            pow_nonce: 0,
            pow_difficulty: 4,
            ttl: 5,
        }
    }

    /// Builder : fixer la difficulté PoW d'un événement signé.
    pub fn with_pow_difficulty(mut self, difficulty: u8) -> Self {
        self.pow_difficulty = difficulty;
        self
    }

    /// Canonical SHA-256 event ID over the immutable fields:
    /// `[pubkey, created_at, kind as u8, tags (stable order), content]`.
    /// All fields are part of the serialization, so any difference in
    /// created_at or kind yields a different ID.
    fn compute_id(
        pubkey: &str,
        created_at: u64,
        kind: &OndeMessageType,
        tags: &[String],
        content: &str,
    ) -> String {
        let canonical =
            serde_json::json!([pubkey, created_at, Self::kind_code(kind), tags, content]);
        let data = serde_json::to_vec(&canonical).unwrap();
        hex::encode(Sha256::digest(&data))
    }

    /// Stable numeric code for an event kind
    fn kind_code(kind: &OndeMessageType) -> u8 {
        match kind {
            OndeMessageType::Alert => 0,
            OndeMessageType::MutualAid => 1,
            OndeMessageType::VoiceMemo => 2,
            OndeMessageType::Transcription => 3,
            OndeMessageType::Transaction => 4,
            OndeMessageType::AiQuery => 5,
            OndeMessageType::AiResponse => 6,
            OndeMessageType::FileShareRequest => 7,
            OndeMessageType::Heartbeat => 8,
            // Phase 1.1 : codes 9..12 — aucun code existant n'est renuméroté,
            // le format wire des types antérieurs reste stable.
            OndeMessageType::UpdateAnnounce => 9,
            OndeMessageType::UpdateManifest => 10,
            OndeMessageType::UpdateChunk => 11,
            OndeMessageType::UpdateRequest => 12,
            // Phase 1.2 : code 13 — nouvel endossement WoT propagé dans le
            // gossip, toujours sans renumérotation des types existants.
            OndeMessageType::Endorsement => 13,
            // Phase 1.4 : code 14 — annonce de rotation d'identité X25519
            // (forward secrecy du chiffrement point-à-point). L'identité
            // stable de signature ne change PAS (réputation intacte) ; seule
            // la clé X25519 de chiffrement tourne.
            OndeMessageType::IdentityRotation => 14,
        }
    }

    /// Minimum PoW difficulty accepted by a receiver.
    ///
    /// `pow_difficulty = 0` → prefix `""` → `verify_pow` always true → free
    /// spam. The difficulty field is NOT part of the canonical ID (not
    /// signed), so a malicious sender can set it to 0 without breaking the
    /// signature. The receiver therefore enforces a floor: difficulty 1 means
    /// at least 2 hash attempts on average — trivial for an honest client, a
    /// real (if small) cost for a spammer. Honest producers already use 2–4.
    pub const MIN_POW_DIFFICULTY: u8 = 1;

    /// Verify content validity: size limit, timestamp sanity, canonical ID,
    /// Ed25519 signature and PoW (plancher réseau fixe).
    pub fn validate(&self) -> Result<(), String> {
        self.validate_with_pow_min(Self::MIN_POW_DIFFICULTY)
    }

    /// Verify content validity avec un plancher PoW **adaptatif** basé sur la
    /// réputation de l'expéditeur (Audit #11).
    ///
    /// - Nœud de confiance (score >= TRUSTED_THRESHOLD) : aucun PoW requis
    ///   (difficulté 0 autorisée).
    /// - Nœud inconnu : `MAX_POW_DIFFICULTY` requis (coût CPU significatif).
    /// - Nœud intermédiaire : difficulté linéaire selon sa réputation.
    ///
    /// Remplace le coût CPU fixe ~65k SHA-256/message par un coût concentré
    /// sur les nœuds qui n'ont pas encore prouvé leur fiabilité.
    pub fn validate_with_reputation(
        &self,
        reputation: &crate::reputation::ReputationSystem,
    ) -> Result<(), String> {
        let required = reputation.required_pow_difficulty(&self.pubkey);
        self.validate_with_pow_min(required)
    }

    /// Contrôles communs + plancher de difficulté PoW paramétrable.
    fn validate_with_pow_min(&self, min_difficulty: u8) -> Result<(), String> {
        // Alert size limit — compté en caractères (pas en octets) pour
        // éviter de tronquer les caractères multioctets (Audit m1).
        if let OndeMessageType::Alert = &self.kind {
            if self.content.chars().count() > MAX_ALERT_SIZE {
                return Err(format!("Alert exceeds {} character limit", MAX_ALERT_SIZE));
            }
        }

        // Reject timestamps too far in the future (clock skew > 5 minutes)
        let now = now_secs();
        if self.created_at > now && self.created_at - now > MAX_CLOCK_SKEW_SECS {
            return Err("Event created_at is too far in the future".to_string());
        }

        // The ID must match the canonical serialization of the fields
        let expected_id = Self::compute_id(
            &self.pubkey,
            self.created_at,
            &self.kind,
            &self.tags,
            &self.content,
        );
        if self.id != expected_id {
            return Err("Event ID does not match canonical serialization".to_string());
        }

        // Signature must be present and valid (Ed25519 over the canonical ID)
        if self.sig.is_empty() {
            return Err("Missing signature".to_string());
        }
        let pubkey_bytes =
            decode_hex_32(&self.pubkey).map_err(|_| "Invalid pubkey encoding".to_string())?;
        let sig_bytes =
            decode_hex_64(&self.sig).map_err(|_| "Invalid signature encoding".to_string())?;
        if !Identity::verify_from_pubkey(&pubkey_bytes, self.id.as_bytes(), &sig_bytes) {
            return Err("Invalid signature".to_string());
        }

        // Enforce the required PoW difficulty floor BEFORE the hash check.
        // Without this, `pow_difficulty = 0` → prefix `""` → `starts_with("")`
        // is always true → a malicious sender can flood the mesh for free.
        // The difficulty is NOT part of the canonical ID (not signed), so an
        // attacker can freely set it to 0 without breaking the signature.
        if self.pow_difficulty < min_difficulty {
            return Err(format!(
                "PoW difficulty {diff} is below network minimum (required {min})",
                diff = self.pow_difficulty,
                min = min_difficulty
            ));
        }

        // Verify PoW (the ID is stable — PoW is checked against hash(id:nonce)).
        // Note: difficulty 0 → prefix "" → toujours vrai (cas des nœuds de
        // confiance, qui n'ont pas de coût PoW).
        if !Self::verify_pow(&self.id, self.pow_nonce, self.pow_difficulty) {
            return Err("Invalid PoW".to_string());
        }

        Ok(())
    }

    /// Verify proof of work
    fn verify_pow(event_id: &str, nonce: u64, difficulty: u8) -> bool {
        let data = format!("{event_id}:{nonce}");
        let hash = Sha256::digest(data.as_bytes());
        let hex = format!("{hash:x}");
        let zeros = difficulty;
        hex.starts_with(&"0".repeat(zeros as usize))
    }

    /// Compute PoW for this event.
    ///
    /// The ID is STABLE — PoW never changes it. The nonce is stored separately
    /// and verified via hash(id:nonce), so the canonical ID and signature stay
    /// valid regardless of PoW.
    pub fn compute_pow(&mut self, max_iterations: u64) -> bool {
        let target = "0".repeat(self.pow_difficulty as usize);

        for nonce in 0..max_iterations {
            let data = format!("{}:{nonce}", self.id);
            let hash = Sha256::digest(data.as_bytes());
            let hex = format!("{hash:x}");

            if hex.starts_with(&target) {
                self.pow_nonce = nonce;
                return true;
            }
        }

        false
    }

    /// Check if this event is expired (saturating — never underflows)
    pub fn is_expired(&self, max_age_sec: u64) -> bool {
        let now = now_secs();
        now.saturating_sub(self.created_at) > max_age_sec
    }

    /*
     * Sérialisation wire (Phase 1.3 — TrafficPadding opérationnel sur le flux).
     *
     * Choix de câblage (documenté) : le padding est appliqué au point le plus
     * **centralisé** du flux réseau, la **sérialisation** du `MeshEvent` en
     * octets, plutôt que dans `MeshTransport::send`/`send_best`. Raison : le
     * trait `MeshTransport` n'est PAS encore branché au gossip (le flux réel
     * passe par `GossipProtocol` + `Node`), donc envelopper `send` laisserait
     * le padding inopérant — c'était précisément l'état initial (dead code).
     * Ici, TOUT octet émis vers un pair passe par `to_wire_bytes` (pad) et
     * TOUT octet entrant est traité par `from_wire_bytes` (unpad) AVANT tout
     * décodage/validation. Le format est binaire compact (voir le schéma) :
     * un message texte de 100 B tient ainsi dans le seau minimal de 256 B.
     *
     * Format wire (petit-boutiste) :
     *   id (32) || pubkey (32) || created_at (u64) || kind (u8) ||
     *   tags_count (u32) || [ tag_len (u32) || tag… ]* ||
     *   content_len (u32) || content (utf8) || sig (64) ||
     *   pow_nonce (u64) || pow_difficulty (u8) || ttl (u8)
     * le tout suffixé par `TrafficPadding::pad` (zéros jusqu'au seau).
     */

    /// Sérialiser l'événement en octets **padés** (format wire compact).
    ///
    /// C'est le point d'émission unique du flux réseau : chaque octet sortant
    /// vers un pair est produit ici, donc padé au seau. Échoue proprement si
    /// l'événement n'est pas encodable (id/pubkey/sig hex invalides) — jamais
    /// de panique.
    pub fn to_wire_bytes(&self) -> Result<Vec<u8>, String> {
        let id = decode_hex_32(&self.id).map_err(|_| "wire: invalid event id".to_string())?;
        let pubkey =
            decode_hex_32(&self.pubkey).map_err(|_| "wire: invalid pubkey encoding".to_string())?;
        let sig = decode_hex_64(&self.sig).map_err(|_| "wire: invalid signature".to_string())?;

        let mut out = Vec::with_capacity(32 + 32 + 8 + 1 + 4 + 4 + self.content.len() + 64 + 8 + 2);
        out.extend_from_slice(&id);
        out.extend_from_slice(&pubkey);
        out.extend_from_slice(&self.created_at.to_le_bytes());
        out.push(Self::kind_code(&self.kind));
        out.extend_from_slice(&(self.tags.len() as u32).to_le_bytes());
        for tag in &self.tags {
            out.extend_from_slice(&(tag.len() as u32).to_le_bytes());
            out.extend_from_slice(tag.as_bytes());
        }
        out.extend_from_slice(&(self.content.len() as u32).to_le_bytes());
        out.extend_from_slice(self.content.as_bytes());
        out.extend_from_slice(&sig);
        out.extend_from_slice(&self.pow_nonce.to_le_bytes());
        out.push(self.pow_difficulty);
        out.push(self.ttl);

        Ok(TrafficPadding::pad(&out))
    }

    /// Désérialiser un événement depuis des octets wire **padés ou non**.
    ///
    /// C'est le point de réception unique. Le format wire est **auto-descriptif**
    /// (champs à taille fixe ou préfixés par leur longueur ; `WireReader::take`
    /// lit exactement la taille demandée et rejette tout excédent tronqué), donc
    /// le lecteur est **indifférent au padding** : les zéros de seau en fin de
    /// message sont simplement ignorés, et il n'est PAS nécessaire de les retirer
    /// avant décodage. On ne fait donc **pas** d'`unpad` (stripping des zéros de
    /// fin) ici — celui-ci corrompait tout message valide dont le dernier octet
    /// réel est `0x00` (ex. `ttl = 0`) en le rognant jusqu'à la troncature.
    ///
    /// Une entrée vide ou tronquée (dans le contenu réel) retourne une erreur,
    /// jamais une panique.
    pub fn from_wire_bytes(data: &[u8]) -> Result<Self, String> {
        // Garde anti-paquet nul : un paquet entièrement nul n'est jamais un
        // événement signé légitime (id = hachage non nul, signature non nulle).
        // Sans `unpad` le reader se-delimiterait sur ces zéros et produirait un
        // événement dégénéré (kind=0, id/sig=0) ; on le rejette explicitement.
        if !data.is_empty() && data.iter().all(|b| *b == 0) {
            return Err("wire: all-zero packet is not a valid event".to_string());
        }
        // Pas d'`unpad` : le format est auto-descriptif, le reader s'arrête au
        // dernier champ (ttl) et ignore le padding de seau qui suit.
        let mut r = WireReader::new(data);

        let id = hex::encode(r.take_array::<32>()?);
        let pubkey = hex::encode(r.take_array::<32>()?);
        let created_at = r.take_u64()?;
        let kind = Self::kind_from_code(r.take_u8()?)?;
        let tags_len = r.take_u32()?;
        if tags_len > 10_000 {
            return Err(format!("wire: too many tags ({tags_len})"));
        }
        let mut tags = Vec::with_capacity(tags_len as usize);
        for _ in 0..tags_len {
            tags.push(r.take_string()?);
        }
        let content = r.take_string()?;
        let sig = hex::encode(r.take_array::<64>()?);
        let pow_nonce = r.take_u64()?;
        let pow_difficulty = r.take_u8()?;
        let ttl = r.take_u8()?;

        Ok(Self {
            id,
            pubkey,
            created_at,
            kind,
            tags,
            content,
            sig,
            pow_nonce,
            pow_difficulty,
            ttl,
        })
    }

    /// Retrouver le type d'événement depuis son code wire (récepteur).
    fn kind_from_code(code: u8) -> Result<OndeMessageType, String> {
        Ok(match code {
            0 => OndeMessageType::Alert,
            1 => OndeMessageType::MutualAid,
            2 => OndeMessageType::VoiceMemo,
            3 => OndeMessageType::Transcription,
            4 => OndeMessageType::Transaction,
            5 => OndeMessageType::AiQuery,
            6 => OndeMessageType::AiResponse,
            7 => OndeMessageType::FileShareRequest,
            8 => OndeMessageType::Heartbeat,
            9 => OndeMessageType::UpdateAnnounce,
            10 => OndeMessageType::UpdateManifest,
            11 => OndeMessageType::UpdateChunk,
            12 => OndeMessageType::UpdateRequest,
            13 => OndeMessageType::Endorsement,
            // Phase 1.4 : annonce de rotation d'identité X25519.
            14 => OndeMessageType::IdentityRotation,
            other => return Err(format!("wire: unknown kind code {other}")),
        })
    }
}

/// Decode a 32-byte hex string
fn decode_hex_32(s: &str) -> Result<[u8; 32], ()> {
    let bytes = hex::decode(s).map_err(|_| ())?;
    if bytes.len() != 32 {
        return Err(());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Decode a 64-byte hex string
fn decode_hex_64(s: &str) -> Result<[u8; 64], ()> {
    let bytes = hex::decode(s).map_err(|_| ())?;
    if bytes.len() != 64 {
        return Err(());
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Lecteur d'octets à bornes explicites pour le format wire (Phase 1.3).
///
/// Retourne une erreur sur entrée tronquée ou vide au lieu de paniquer —
/// exigence de robustesse du padding (pas de panique sur entrée vide).
struct WireReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> WireReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.buf.len() {
            return Err(format!(
                "wire message truncated (need {n} bytes at offset {}, have {})",
                self.pos,
                self.buf.len() - self.pos
            ));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn take_array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let s = self.take(N)?;
        s.try_into()
            .map_err(|_| "wire: length mismatch".to_string())
    }

    fn take_u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn take_u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take_array()?))
    }

    fn take_u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take_array()?))
    }

    fn take_string(&mut self) -> Result<String, String> {
        let len = self.take_u32()? as usize;
        let b = self.take(len)?;
        String::from_utf8(b.to_vec()).map_err(|e| format!("wire content is not valid UTF-8: {e}"))
    }
}

/*
 * Gossip Protocol for Public Feed
 */

/// Nombre maximal d'événements connus conservés en mémoire (Audit M3).
/// Au-delà, les plus anciens sont évincés — les pairs qui les redemanderont
/// devront les re-récupérer ailleurs, mais la mémoire du nœud reste bornée.
pub const MAX_KNOWN_EVENTS: usize = 10_000;

/// Taille maximale de la file de diffusion (outbox) du gossip (Audit M3).
pub const MAX_PENDING_BROADCASTS: usize = 1_000;

/// Nombre maximal d'IDs d'événements mémorisés **par pair** pour éviter de
/// lui renvoyer des événements déjà livrés (Audit M3).
pub const MAX_DELIVERED_PER_PEER: usize = 2_000;

/// Gossip protocol state.
///
/// **Corrigé (Audit M3)** : `get_pending_for_peer` ne vide plus la file
/// globale — chaque pair reçoit uniquement les événements qui ne lui ont pas
/// encore été livrés, et la mémoire est bornée (`MAX_KNOWN_EVENTS`,
/// `MAX_PENDING_BROADCASTS`, `MAX_DELIVERED_PER_PEER`).
pub struct GossipProtocol {
    known_events: std::collections::HashSet<String>,
    pending_broadcasts: std::collections::VecDeque<MeshEvent>,
    /// peer_id → IDs des événements déjà envoyés à ce pair
    delivered: std::collections::HashMap<String, std::collections::VecDeque<String>>,
}

impl GossipProtocol {
    pub fn new() -> Self {
        Self {
            known_events: std::collections::HashSet::new(),
            pending_broadcasts: std::collections::VecDeque::new(),
            delivered: std::collections::HashMap::new(),
        }
    }

    /// Process a new event from the local user.
    ///
    /// The event is validated first — invalid events are refused.
    /// Returns `Ok(true)` if added, `Ok(false)` if already known,
    /// `Err(reason)` if the event is invalid.
    pub fn add_event(&mut self, event: MeshEvent) -> Result<bool, String> {
        event.validate()?;
        self.insert_new(event)
    }

    /// Add an event validated avec le plancher PoW **adaptatif** de la
    /// réputation (Audit #11) — permet aux nœuds de confiance de poster
    /// avec une difficulté 0 sans être rejetés par le plancher réseau fixe.
    pub fn add_event_with_reputation(
        &mut self,
        event: MeshEvent,
        reputation: &crate::reputation::ReputationSystem,
    ) -> Result<bool, String> {
        event.validate_with_reputation(reputation)?;
        self.insert_new(event)
    }

    /// Process event received from peer
    pub fn receive_event(&mut self, event: MeshEvent, _peer_id: &str) -> bool {
        if self.known_events.contains(&event.id) {
            return false; // Duplicate
        }

        if event.validate().is_ok() {
            self.insert_new(event).unwrap_or(false)
        } else {
            false
        }
    }

    /// Enregistre un événement connu et le place dans l'outbox, avec bornes.
    fn insert_new(&mut self, event: MeshEvent) -> Result<bool, String> {
        if !self.known_events.insert(event.id.clone()) {
            return Ok(false); // Déjà connu
        }
        // Borne mémoire : évince le plus ancien événement en attente
        if self.pending_broadcasts.len() >= MAX_PENDING_BROADCASTS {
            self.pending_broadcasts.pop_front();
        }
        self.pending_broadcasts.push_back(event);
        // Borne mémoire sur les événements connus (FIFO)
        if self.known_events.len() > MAX_KNOWN_EVENTS {
            self.expire_old_known();
        }
        Ok(true)
    }

    /// Get events to broadcast to a specific peer — sans vider l'outbox
    /// globale (Audit M3).
    ///
    /// Ne renvoie que les événements qui n'ont **pas encore** été envoyés à
    /// ce pair (suivi par-pair `delivered`), puis les marque comme livrés.
    /// Un événement reste dans l'outbox jusqu'à éviction (bornée) afin que
    /// les autres pairs puissent aussi le recevoir.
    pub fn get_pending_for_peer(&mut self, peer_id: &str) -> Vec<MeshEvent> {
        let delivered_set = self.delivered.entry(peer_id.to_string()).or_default();

        let mut to_send = Vec::new();
        for event in self.pending_broadcasts.iter() {
            if !delivered_set.iter().any(|id| id == &event.id) {
                to_send.push(event.clone());
            }
        }

        // Marque les événements envoyés comme livrés à ce pair (borné)
        for event in &to_send {
            if delivered_set.len() >= MAX_DELIVERED_PER_PEER {
                delivered_set.pop_front();
            }
            delivered_set.push_back(event.id.clone());
        }

        to_send
    }

    /// Même sélection que [`GossipProtocol::get_pending_for_peer`], mais
    /// chaque événement est **sérialisé et padé** via
    /// [`MeshEvent::to_wire_bytes`] (Phase 1.3).
    ///
    /// C'est le point d'émission centralisé du flux réseau : les octets
    /// produits ici sont exactement ceux qui sortent vers le pair, donc
    /// systématiquement padés au seau. Le receveur les traite avec
    /// [`MeshEvent::from_wire_bytes`] (unpad avant décodage).
    pub fn get_pending_for_peer_wire(&mut self, peer_id: &str) -> Result<Vec<Vec<u8>>, String> {
        self.get_pending_for_peer(peer_id)
            .into_iter()
            .map(|event| event.to_wire_bytes())
            .collect()
    }

    pub fn known_count(&self) -> usize {
        self.known_events.len()
    }

    /// Get pending broadcasts (outbox, tous événements confondus)
    pub fn get_pending_broadcasts(&self) -> Vec<&MeshEvent> {
        self.pending_broadcasts.iter().collect()
    }

    /// Évince les événements connus les plus anciens au-delà de la borne.
    fn expire_old_known(&mut self) {
        // Réinsère les événements connus les plus récents (l'ordre d'insertion
        // dans la HashSet n'est pas garanti : on repart de l'outbox qui est FIFO).
        if self.known_events.len() <= MAX_KNOWN_EVENTS {
            return;
        }
        // Construit un ensemble de référence à partir de l'outbox (les plus
        // récents), puis évince les autres connus jusqu'à la borne.
        let keep: std::collections::HashSet<String> = self
            .pending_broadcasts
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let overflow = self.known_events.len() - MAX_KNOWN_EVENTS;
        let mut evicted = 0usize;
        let ids: Vec<String> = self.known_events.iter().cloned().collect();
        for id in ids {
            if evicted >= overflow {
                break;
            }
            if !keep.contains(&id) {
                self.known_events.remove(&id);
                evicted += 1;
            }
        }
    }
}

impl Default for GossipProtocol {
    fn default() -> Self {
        Self::new()
    }
}

/*
 * Voice Memo — Opus Codec Wrapper
 */

/// Encapsulated voice memo
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceMemo {
    pub event_id: String,
    /// Opus encoded audio at 8kbps
    pub opus_data: Vec<u8>,
    /// Duration in seconds
    pub duration_sec: f32,
    /// Auto-transcribed text (filled at receive)
    pub transcription: Option<String>,
}

impl VoiceMemo {
    pub fn new(event_id: String, opus_data: Vec<u8>, duration_sec: f32) -> Self {
        Self {
            event_id,
            opus_data,
            duration_sec,
            transcription: None,
        }
    }

    /// Estimated size at 8kbps (1000 bytes/sec)
    pub fn estimated_size_bytes(&self) -> usize {
        (self.duration_sec * 1000.0) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = MeshEvent::new(
            "pubkey123",
            OndeMessageType::Alert,
            "Test alert".to_string(),
            vec![],
        );
        assert_eq!(event.pubkey, "pubkey123");
        assert!(event.pow_nonce == 0); // Not computed yet
                                       // Unsigned events are not valid
        assert!(event.sig.is_empty());
    }

    #[test]
    fn test_pow_verify() {
        // difficulty 2 is easy to find
        assert!(MeshEvent::verify_pow("test-id", 0, 1));
    }

    #[test]
    fn test_alert_size_limit() {
        let event = MeshEvent::new(
            "key",
            OndeMessageType::Alert,
            "x".repeat(MAX_ALERT_SIZE + 1),
            vec![],
        );
        assert!(event.validate().is_err());
    }

    #[test]
    fn test_signed_event_validates() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "hello".to_string(),
            vec!["test".to_string()],
        );
        event.pow_difficulty = 2; // above the network minimum, PoW still trivial
        assert!(
            event.compute_pow(1_000_000),
            "PoW nonce must be found at difficulty 2"
        );

        assert!(event.validate().is_ok(), "Signed event must validate");
        assert_eq!(event.sig.len(), 128); // 64 bytes of hex
        assert_eq!(event.pubkey, identity.pubkey_hex());
    }

    #[test]
    fn test_zero_pow_difficulty_rejected() {
        // The receiver must refuse events below the network minimum difficulty.
        // An attacker can freely set `pow_difficulty = 0` (it is NOT part of
        // the canonical ID, so the signature stays valid) and previously the
        // empty prefix `""` always passed `verify_pow` → free spam.
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "spam".to_string(),
            vec![],
        );
        event.pow_difficulty = 0;
        assert!(event.validate().is_err(), "difficulty 0 must be rejected");
        assert!(
            event
                .validate()
                .unwrap_err()
                .contains("below network minimum"),
            "error must mention the network minimum"
        );

        // The honest floor itself is accepted (PoW trivially satisfiable)
        let mut event2 =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "ok".to_string(), vec![]);
        event2.pow_difficulty = 1; // = MeshEvent::MIN_POW_DIFFICULTY (trivially satisfiable)
        assert!(event2.compute_pow(1_000_000));
        assert!(
            event2.validate().is_ok(),
            "events at the network minimum difficulty must validate"
        );
    }

    #[test]
    fn test_tampered_signature_rejected() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "hello".to_string(),
            vec![],
        );
        event.pow_difficulty = 2;
        assert!(event.compute_pow(1_000_000));
        assert!(event.validate().is_ok());

        // Falsify the signature (flip one bit)
        let mut sig_bytes = hex::decode(&event.sig).unwrap();
        sig_bytes[0] ^= 0x01;
        event.sig = hex::encode(&sig_bytes);

        assert!(
            event.validate().is_err(),
            "Event with a falsified signature must be rejected"
        );
    }

    #[test]
    fn test_unsigned_event_rejected() {
        // new() leaves the signature empty → validate() must refuse
        let event = MeshEvent::new("key", OndeMessageType::Alert, "hello".to_string(), vec![]);
        assert!(
            event.validate().is_err(),
            "Event with an empty signature must be rejected"
        );
    }

    #[test]
    fn test_tampered_content_rejected() {
        let identity = Identity::generate();
        let mut event = MeshEvent::new_signed(
            &identity,
            OndeMessageType::Alert,
            "hello".to_string(),
            vec![],
        );
        event.pow_difficulty = 2;

        // Tamper with content — the canonical ID no longer matches
        event.content = "tampered".to_string();
        assert!(
            event.validate().is_err(),
            "Event whose content does not match its ID must be rejected"
        );
    }

    #[test]
    fn test_canonical_id_distinct() {
        let pk = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let kind = OndeMessageType::Alert;
        let tags = vec!["a".to_string()];
        let content = "same content";

        // Same author + content, created_at shifted by 1s → different IDs
        let id_t = MeshEvent::compute_id(pk, 1_000, &kind, &tags, content);
        let id_t_plus_1 = MeshEvent::compute_id(pk, 1_001, &kind, &tags, content);
        assert_ne!(
            id_t, id_t_plus_1,
            "created_at +1s must yield a different ID"
        );

        // Same author + content + timestamp, different kind → different IDs
        let id_alert = MeshEvent::compute_id(pk, 1_000, &OndeMessageType::Alert, &tags, content);
        let id_voice =
            MeshEvent::compute_id(pk, 1_000, &OndeMessageType::VoiceMemo, &tags, content);
        assert_ne!(
            id_alert, id_voice,
            "different kinds must yield different IDs"
        );

        // Determinism: identical inputs → identical ID
        let id_again = MeshEvent::compute_id(pk, 1_000, &kind, &tags, content);
        assert_eq!(id_t, id_again, "canonical ID must be deterministic");
    }

    #[test]
    fn test_is_expired_no_underflow() {
        let identity = Identity::generate();
        let mut event =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hi".into(), vec![]);
        event.pow_difficulty = 2;

        // An event created in the future must not panic (saturating_sub)
        event.created_at = now_secs() + 60;
        assert!(!event.is_expired(3600));

        // Old event is expired
        event.created_at = now_secs() - 7200;
        assert!(event.is_expired(3600));
    }

    #[test]
    fn test_future_timestamp_rejected() {
        let identity = Identity::generate();
        let mut event =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hi".into(), vec![]);
        event.pow_difficulty = 2;
        assert!(event.compute_pow(1_000_000));
        assert!(event.validate().is_ok());

        // created_at more than 5 minutes in the future → refused
        event.created_at = now_secs() + MAX_CLOCK_SKEW_SECS + 1;
        assert!(
            event.validate().is_err(),
            "Event created too far in the future must be rejected"
        );
    }

    #[test]
    fn test_gossip_dedup() {
        let mut gossip = GossipProtocol::new();
        let identity = Identity::generate();
        let mut event =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hello".into(), vec![]);
        event.pow_difficulty = 2;
        assert!(event.compute_pow(1_000_000));
        let id = event.id.clone();

        assert!(gossip.add_event(event.clone()).is_ok());
        assert_eq!(gossip.known_count(), 1);
        assert_eq!(gossip.get_pending_broadcasts().len(), 1);

        // Duplicate is not added again
        assert!(!gossip.add_event(event.clone()).unwrap());
        assert_eq!(gossip.known_count(), 1);
        assert_eq!(gossip.get_pending_broadcasts().len(), 1);
        assert_eq!(gossip.get_pending_broadcasts()[0].id, id);

        // Invalid (unsigned) event is refused by add_event
        let bad = MeshEvent::new("key", OndeMessageType::Alert, "hello".into(), vec![]);
        assert!(
            gossip.add_event(bad).is_err(),
            "Invalid event must be refused by add_event"
        );
    }

    #[test]
    fn test_gossip_per_peer_delivery_no_global_drain() {
        // Audit M3 : l'ancien `drain(..)` vidait l'outbox globale au premier
        // pair → les pairs suivants ne recevaient RIEN. Le suivi par-pair
        // doit donner à chaque pair tous les événements, sans doublons.
        let mut gossip = GossipProtocol::new();
        let identity = Identity::generate();

        let make_event = |content: &str| {
            let mut e = MeshEvent::new_signed(
                &identity,
                OndeMessageType::Alert,
                content.to_string(),
                vec![],
            );
            e.pow_difficulty = 2;
            assert!(e.compute_pow(1_000_000));
            e
        };

        let e1 = make_event("événement un");
        let e2 = make_event("événement deux");
        assert!(gossip.add_event(e1).is_ok());
        assert!(gossip.add_event(e2).is_ok());
        assert_eq!(gossip.get_pending_broadcasts().len(), 2);

        // Pair A reçoit les deux événements
        let to_a = gossip.get_pending_for_peer("peer-a");
        assert_eq!(to_a.len(), 2);

        // Pair B reçoit AUSSI les deux événements (pas de drain global)
        let to_b = gossip.get_pending_for_peer("peer-b");
        assert_eq!(to_b.len(), 2, "peer B must not be starved by peer A");

        // Un nouvel événement arrive
        let e3 = make_event("événement trois");
        assert!(gossip.add_event(e3).is_ok());

        // Pair A : seulement le NOUVEAU (pas de re-livraison des anciens)
        let to_a2 = gossip.get_pending_for_peer("peer-a");
        assert_eq!(to_a2.len(), 1);
        assert_eq!(to_a2[0].content, "événement trois");

        // Pair B : lui aussi reçoit le nouveau
        let to_b2 = gossip.get_pending_for_peer("peer-b");
        assert_eq!(to_b2.len(), 1);
        assert_eq!(to_b2[0].content, "événement trois");

        // Personne ne reçoit de doublon
        assert!(gossip.get_pending_for_peer("peer-a").is_empty());
        assert!(gossip.get_pending_for_peer("peer-b").is_empty());
    }

    #[test]
    fn test_gossip_bounds_are_enforced() {
        // Audit M3 : la mémoire du gossip doit rester bornée, même avec un
        // flot continu d'événements et beaucoup de pairs.
        let mut gossip = GossipProtocol::new();
        let identity = Identity::generate();

        // Remplit l'outbox au-delà de la borne
        for i in 0..(MAX_PENDING_BROADCASTS + 100) {
            let mut e = MeshEvent::new_signed(
                &identity,
                OndeMessageType::Alert,
                format!("event-{i}"),
                vec![],
            );
            e.pow_difficulty = 2;
            assert!(e.compute_pow(1_000_000));
            gossip.add_event(e).unwrap();
        }
        assert!(
            gossip.get_pending_broadcasts().len() <= MAX_PENDING_BROADCASTS,
            "pending broadcasts must be bounded"
        );
        assert!(gossip.known_count() <= MAX_KNOWN_EVENTS);

        // Le suivi par-pair est borné
        let peer = "peer-heavy";
        for _ in 0..5 {
            gossip.get_pending_for_peer(peer);
        }
        let delivered_len = gossip.delivered.get(peer).map(|d| d.len()).unwrap_or(0);
        assert!(
            delivered_len <= MAX_DELIVERED_PER_PEER,
            "delivered tracking per peer must be bounded"
        );
    }

    #[test]
    fn test_reputation_adaptive_pow_trusted_no_pow() {
        use crate::reputation::ReputationSystem;

        // Un nœud de confiance (genesis) peut poster SANS PoW
        let identity = Identity::generate();
        let mut rep = ReputationSystem::new();
        rep.bootstrap(&[identity.pubkey_hex()]);

        let event =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "alerte".into(), vec![])
                .with_pow_difficulty(0);

        // validate() standard (plancher réseau) le rejette…
        assert!(event.validate().is_err());
        // …mais validate_with_reputation l'accepte (nœud de confiance)
        assert!(
            event.validate_with_reputation(&rep).is_ok(),
            "trusted node must be allowed to post without PoW"
        );
    }

    #[test]
    fn test_reputation_adaptive_pow_unknown_requires_high() {
        use crate::reputation::ReputationSystem;

        // Un nœud INCONNU doit payer un PoW élevé (MAX_POW_DIFFICULTY)
        let rep = ReputationSystem::new();
        let identity = Identity::generate();

        // Difficulté 1 (plancher réseau) : insuffisante pour un inconnu
        let mut weak =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "spam".into(), vec![])
                .with_pow_difficulty(1);
        assert!(weak.compute_pow(1_000_000));
        assert!(
            weak.validate().is_ok(),
            "standard validate accepts difficulty 1"
        );
        assert!(
            weak.validate_with_reputation(&rep).is_err(),
            "unknown node with difficulty 1 must be rejected by reputation"
        );

        // Difficulté MAX : acceptable
        let mut strong =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, "spam".into(), vec![])
                .with_pow_difficulty(crate::reputation::MAX_POW_DIFFICULTY);
        assert!(strong.compute_pow(10_000_000));
        assert!(
            strong.validate_with_reputation(&rep).is_ok(),
            "unknown node paying MAX PoW must be accepted"
        );
    }

    #[test]
    fn test_signed_event_uses_fuzzy_timestamp() {
        // L'horodatage flou (±30 s) reste dans la fenêtre de tolérance
        // MAX_CLOCK_SKEW_SECS → les événements signés restent valides.
        let identity = Identity::generate();
        let event = MeshEvent::new_signed(&identity, OndeMessageType::Alert, "hi".into(), vec![]);
        let now = now_secs();
        assert!(
            event.created_at.abs_diff(now) <= 30,
            "fuzzy timestamp must stay within ±30 s"
        );
    }

    #[test]
    fn test_update_kinds_have_stable_distinct_codes() {
        // Phase 1.1 : les nouveaux types update ont des codes dédiés (9..12)
        // et les codes des types existants restent stables (0..8).
        let codes: Vec<u8> = [
            OndeMessageType::Alert,
            OndeMessageType::MutualAid,
            OndeMessageType::VoiceMemo,
            OndeMessageType::Transcription,
            OndeMessageType::Transaction,
            OndeMessageType::AiQuery,
            OndeMessageType::AiResponse,
            OndeMessageType::FileShareRequest,
            OndeMessageType::Heartbeat,
            OndeMessageType::UpdateAnnounce,
            OndeMessageType::UpdateManifest,
            OndeMessageType::UpdateChunk,
            OndeMessageType::UpdateRequest,
        ]
        .iter()
        .map(MeshEvent::kind_code)
        .collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "all kind codes must be distinct");

        // Des kinds différents produisent des IDs canoniques différents
        // (même contenu, même timestamp, même auteur).
        let pk = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let tags = vec!["peer=x".to_string()];
        let id_announce =
            MeshEvent::compute_id(pk, 42, &OndeMessageType::UpdateAnnounce, &tags, "x");
        let id_manifest =
            MeshEvent::compute_id(pk, 42, &OndeMessageType::UpdateManifest, &tags, "x");
        let id_chunk = MeshEvent::compute_id(pk, 42, &OndeMessageType::UpdateChunk, &tags, "x");
        let id_request = MeshEvent::compute_id(pk, 42, &OndeMessageType::UpdateRequest, &tags, "x");
        assert_ne!(id_announce, id_manifest);
        assert_ne!(id_manifest, id_chunk);
        assert_ne!(id_chunk, id_request);
        assert_ne!(id_request, id_announce);
    }

    #[test]
    fn test_endorsement_kind_has_stable_code() {
        // Phase 1.2 : le nouvel Endorsement a un code dédié (13), distinct de
        // tous les codes existants (0..12) — le format wire reste stable.
        let codes: Vec<u8> = [
            OndeMessageType::Alert,
            OndeMessageType::MutualAid,
            OndeMessageType::VoiceMemo,
            OndeMessageType::Transcription,
            OndeMessageType::Transaction,
            OndeMessageType::AiQuery,
            OndeMessageType::AiResponse,
            OndeMessageType::FileShareRequest,
            OndeMessageType::Heartbeat,
            OndeMessageType::UpdateAnnounce,
            OndeMessageType::UpdateManifest,
            OndeMessageType::UpdateChunk,
            OndeMessageType::UpdateRequest,
            OndeMessageType::Endorsement,
        ]
        .iter()
        .map(MeshEvent::kind_code)
        .collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "all kind codes must be distinct");

        // Le kind Endorsement produit un ID canonique distinct des autres
        // (même contenu, même timestamp, même auteur).
        let pk = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let tags = vec!["peer=x".to_string()];
        let id_alert = MeshEvent::compute_id(pk, 42, &OndeMessageType::Alert, &tags, "x");
        let id_endorsement =
            MeshEvent::compute_id(pk, 42, &OndeMessageType::Endorsement, &tags, "x");
        assert_ne!(id_alert, id_endorsement);
    }

    // ------------------------------------------------------------------
    // Phase 1.3 — TrafficPadding opérationnel sur le flux (format wire)
    // ------------------------------------------------------------------

    fn make_signed_event(content: &str, tags: Vec<String>) -> MeshEvent {
        let identity = Identity::generate();
        let mut event =
            MeshEvent::new_signed(&identity, OndeMessageType::Alert, content.to_string(), tags);
        event.pow_difficulty = 2;
        assert!(event.compute_pow(1_000_000));
        event
    }

    #[test]
    fn test_wire_roundtrip_signed_event() {
        // Sérialisation padée → désérialisation (unpad) : événement identique,
        // champ par champ.
        let event = make_signed_event(
            "alerte réseau : inondation secteur 3",
            vec!["geohash=u09tunq".to_string(), "peer=abc".to_string()],
        );
        let wire = event.to_wire_bytes().expect("valid event must serialize");
        // Le wire est padé au seau : jamais la taille réelle du JSON/événement.
        assert_eq!(wire.len(), TrafficPadding::bucket_for(wire.len()));

        let decoded = MeshEvent::from_wire_bytes(&wire).expect("padded wire must decode");
        assert_eq!(decoded.id, event.id);
        assert_eq!(decoded.pubkey, event.pubkey);
        assert_eq!(decoded.created_at, event.created_at);
        assert_eq!(decoded.kind, event.kind);
        assert_eq!(decoded.tags, event.tags);
        assert_eq!(decoded.content, event.content);
        assert_eq!(decoded.sig, event.sig);
        assert_eq!(decoded.pow_nonce, event.pow_nonce);
        assert_eq!(decoded.pow_difficulty, event.pow_difficulty);
        assert_eq!(decoded.ttl, event.ttl);
    }

    #[test]
    fn test_wire_accepts_non_padded_input() {
        // Un receveur peut recevoir des octets NON padés (pair legacy ou
        // message plus gros que le seau maximal) : `unpad` les retourne tels
        // quels et le décodage réussit.
        let event = make_signed_event("sans padding", vec![]);
        let padded = event.to_wire_bytes().unwrap();
        let unpadded = TrafficPadding::unpad(&padded);
        assert_ne!(
            unpadded.len(),
            padded.len(),
            "the test must exercise real padding"
        );
        let decoded = MeshEvent::from_wire_bytes(unpadded).expect("non-padded wire must decode");
        assert_eq!(decoded.content, event.content);
        assert_eq!(decoded.id, event.id);
    }

    #[test]
    fn test_wire_empty_and_truncated_no_panic() {
        // Exigence Phase 1.3 : pas de panique sur entrée vide ou tronquée.
        assert!(
            MeshEvent::from_wire_bytes(&[]).is_err(),
            "empty wire must be an error"
        );
        let event = make_signed_event("tronqué", vec![]);
        let padded = event.to_wire_bytes().unwrap();
        let full = TrafficPadding::unpad(&padded);

        // Tronquer DANS le contenu réel (hors padding) → erreur propre. NB :
        // tronquer uniquement des zéros de padding est toléré (unpad retrouve
        // le message complet — propriété auto-descriptive du format).
        assert!(
            MeshEvent::from_wire_bytes(&full[..full.len() - 5]).is_err(),
            "truncated wire (into the payload) must be an error"
        );
        // Un paquet entièrement de zéros (padding seul) est aussi une erreur.
        assert!(
            MeshEvent::from_wire_bytes(&[0u8; 256]).is_err(),
            "all-zero wire must be an error"
        );
    }

    #[test]
    fn test_wire_100b_alert_pads_to_256() {
        // L'exigence du brief : une alerte de 100 B observée sur le fil fait
        // exactement 256 B (seau minimal), jamais 100 B.
        let content = "x".repeat(100);
        let event = make_signed_event(&content, vec![]);
        assert_eq!(event.content.len(), 100);
        let wire = event.to_wire_bytes().unwrap();
        assert_eq!(
            wire.len(),
            256,
            "100 B content must be padded to the 256 B bucket"
        );
        let decoded = MeshEvent::from_wire_bytes(&wire).unwrap();
        assert_eq!(decoded.content, content);
    }

    #[test]
    fn test_wire_trailing_zero_ttl_survives_roundtrip() {
        // RÉGRESSION (Aikido PR #8) : `from_wire_bytes` ne doit PAS rogner les
        // zéros de fin. Un événement dont le dernier octet réel est `0x00`
        // (ici `ttl = 0`) doit survivre au round-trip padé. Avant la correction,
        // `TrafficPadding::unpad` supprimait les zéros de fin et tronquait le
        // dernier octet (ttl) du message valide.
        let mut event = make_signed_event("alerte ttl zéro", vec![]);
        event.ttl = 0; // dernier octet réel = 0x00
        let wire = event.to_wire_bytes().unwrap();
        // Le fil est padé : le dernier octet du message réel est 0x00, suivi de
        // zéros de seau — impossible de distinguer « zéro réel » de « zéro
        // padding » sans se fier au format auto-descriptif.
        let decoded = MeshEvent::from_wire_bytes(&wire).expect("ttl=0 event must decode");
        assert_eq!(
            decoded.ttl, 0,
            "a real trailing 0x00 (ttl=0) must not be stripped"
        );
        assert_eq!(decoded.content, event.content);
        assert_eq!(decoded.id, event.id);
    }

    #[test]
    fn test_gossip_pending_wire_pads() {
        // L'outbox du gossip émet des octets padés vers chaque pair.
        let mut gossip = GossipProtocol::new();
        let event = make_signed_event(&"x".repeat(100), vec![]);
        assert!(gossip.add_event(event).is_ok());

        let wire = gossip
            .get_pending_for_peer_wire("peer-b")
            .expect("valid outbox event must serialize");
        assert_eq!(wire.len(), 1);
        assert_eq!(
            wire[0].len(),
            256,
            "gossip emission is padded to the bucket"
        );

        // Livré une fois → plus rien à émettre (tracking par pair conservé).
        assert!(gossip
            .get_pending_for_peer_wire("peer-b")
            .unwrap()
            .is_empty());
        // Un autre pair reçoit encore l'événement (pas de drain global, Audit M3).
        let other = gossip.get_pending_for_peer_wire("peer-c").unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].len(), 256);
    }
}
