//! Protocole de distribution sécurisée d'APK par le mesh (Audit #12/#13).
//!
//! Flux complet d'une mise à jour applicative entre deux nœuds du réseau :
//!
//! ```text
//!   Annonceur (détenteur de la version)        Receveur (nœud à mettre à jour)
//!   ───────────────────────────────────        ────────────────────────────────
//!   1. signe l'UpdateAnnouncement (racine)
//!   2. diffuse  announcement ───────────────►  3. vérifie la signature racine
//!                                                et exige version > locale
//!   4. signe l'ApkManifest canonique (racine)
//!   5. reçoit   request_manifest ◄───────────  6. envoie  request_manifest
//!   7. diffuse  manifest + métadonnées ──────► 8. vérifie la signature racine
//!                                                + hash = hash annoncé ;
//!                                                initialise le transfert
//!   9. découpe  APK en chunks
//!  10. reçoit   request_chunk(n) ◄──────────  11. envoie   request_chunk(n)
//!  12. envoie   chunk(n) ───────────────────► 13. valide index et taille,
//!                                                assemble les chunks
//!  14.                             APK complet → verify_apk_signature()
//!                                                (Ed25519 racine épinglée +
//!                                                 SHA-256 du fichier entier)
//!  15.                              APK valide → installation (ou rejet +
//!                                                purge du transfert)
//! ```
//!
//! La sécurité repose sur trois garde-fous cumulatifs :
//!
//! 1. **L'annonce et le manifeste sont signés par la clé racine épinglée**
//!    (`root_pubkey` fournie au constructeur) — un annonceur inconnu ne peut
//!    pas proposer de mise à jour ;
//! 2. **Le manifeste est lié à l'annonce** (même hash SHA-256 et même version)
//!    — pas de downgrade ni d'injection d'un APK non annoncé ;
//! 3. **L'APK reçu est vérifié de bout en bout** par
//!    [`crate::crypto::verify_apk_signature`] avant toute installation — un
//!    APK falsifié (hash différent) ou signé par une clé inconnue est rejeté.
//!
//! Le manifeste signé du protocole est **exactement** le manifeste canonique
//! [`crate::crypto::ApkManifest`] (magic `ONDEAPK1`, 80 octets) : la
//! vérification de bout en bout réutilise donc la primitive
//! [`crate::crypto::verify_apk_signature`] telle quelle. La taille de l'APK et
//! la taille de chunk sont des **métadonnées de transfert non signées**,
//! ajoutées après le blob signé : elles ne servent qu'à piloter le
//! téléchargement et sont, de fait, liées au hash signé — un pair qui les
//! falsifie provoque au pire un transfert invalide (rejeté), jamais une
//! installation non authentifiée.
//!
//! Le transport (BLE, Wi-Fi Aware…) est volontairement abstrait : ce module
//! expose les messages signés et la machine à états, et c'est l'appelant qui
//! fournit les octets reçus. L'installation réelle sur Android relève de la
//! plateforme (PackageInstaller) ; ce module fournit
//! [`UpdateProtocol::install_verified`] comme point d'intégration qui écrit
//! l'APK vérifié sur disque et enregistre la version installée.
//!
//! Limites de sûreté : taille APK plafonnée à [`MAX_APK_SIZE`], nombre de
//! chunks plafonné à [`MAX_CHUNKS`] (≈ 6 400 chunks de 16 Kio).
//!
//! # Intégration réseau (Phase 1.1 — câblée)
//!
//! Ce module expose la machine à états et les messages signés, sans dépendre
//! de la couche transport ([`crate::network`], gossip, BLE, Wi-Fi Aware…).
//! Le câblage dans le **flux gossip** est réalisé côté `Node`
//! ([`crate::node`], Phase 1.1) : les blobs signés (base64) circulent dans
//! `MeshEvent.content` avec des types dédiés
//! (`OndeMessageType::UpdateAnnounce/Manifest/Chunk/Request`), les
//! métadonnées et la signature racine dans `tags` (`k=v`). Points d'appel
//! réalisés :
//!
//! ```text
//! Réception d'une annonce pair (message OndeMessageType::UpdateAnnounce)
//!   └─ UpdateProtocol::handle_announcement(data, signature)
//!        • signature racine vérifiée, version > locale exigée
//!        • si Ok(announcement) → requête manifeste vers le pair
//! Réception du manifeste (message OndeMessageType::UpdateManifest)
//!   └─ UpdateProtocol::handle_manifest(manifest_bytes, peer)
//!        • hash = hash annoncé, sinon UpdateError::ManifestMismatch
//! Réception d'un chunk (message OndeMessageType::UpdateChunk { index, data })
//!   └─ UpdateProtocol::handle_chunk(index, data)
//!        • valide index/size, assemble, puis assemble_and_verify()
//! Installation
//!   └─ UpdateProtocol::install_verified(apk, path, now)  (desktop)
//!      ou UpdateProtocol::record_install(...)             (Android/PackageInstaller)
//! ```
//!
//! Le routage des messages "update" dans le gossip (types wire partagés +
//! validation PoW adaptative) a été réalisé en Phase 1.1 dans
//! [`crate::node::Node::handle_incoming_update`] / [`crate::node::Node::announce_update`],
//! sans renuméroter les kind_code existants (nouveaux codes 9..12).

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::crypto::{verify_apk_signature, ApkManifest, Identity};

/// Magic des annonces de mise à jour signées.
pub const ANNOUNCEMENT_MAGIC: &[u8; 8] = b"ONDENEW1";
/// Taille de chunk par défaut (16 Kio — adapté au BLE à ~2 Mbit/s).
pub const DEFAULT_CHUNK_SIZE: u32 = 16 * 1024;
/// Plafond de taille d'APK accepté (200 Mio).
pub const MAX_APK_SIZE: u64 = 200 * 1024 * 1024;
/// Plafond de nombre de chunks (garde-fou mémoire côté receveur).
pub const MAX_CHUNKS: usize = 12_800;
/// Taille de la partie signée du manifeste de mise à jour (= `ApkManifest`).
pub const SIGNED_MANIFEST_LEN: usize = 8 + 32 + 32 + 8;
/// Taille du message manifeste complet (blob signé + métadonnées).
pub const MANIFEST_WIRE_LEN: usize = SIGNED_MANIFEST_LEN + 8 + 4;

/// Version sémantique simple (majeure.mineure.patch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parser `"1.2.3"` (exige les trois composantes numériques).
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut parts = s.trim().split('.');
        let major = parts
            .next()
            .ok_or_else(|| "missing major version".to_string())?;
        let minor = parts
            .next()
            .ok_or_else(|| "missing minor version".to_string())?;
        let patch = parts
            .next()
            .ok_or_else(|| "missing patch version".to_string())?;
        if parts.next().is_some() {
            return Err("too many version components".to_string());
        }
        let parse = |v: &str, what: &str| {
            v.parse::<u16>()
                .map_err(|_| format!("invalid {what} version component: {v}"))
        };
        Ok(Self::new(
            parse(major, "major")?,
            parse(minor, "minor")?,
            parse(patch, "patch")?,
        ))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Annonce signée d'une nouvelle version disponible.
///
/// Format signé : `ONDENEW1 || major(2) || minor(2) || patch(2) ||
/// sha256(apk, 32) || timestamp(8)` — 54 octets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAnnouncement {
    pub version: Version,
    pub apk_sha256: [u8; 32],
    pub timestamp: u64,
}

impl UpdateAnnouncement {
    pub fn to_signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 6 + 32 + 8);
        out.extend_from_slice(ANNOUNCEMENT_MAGIC);
        out.extend_from_slice(&self.version.major.to_le_bytes());
        out.extend_from_slice(&self.version.minor.to_le_bytes());
        out.extend_from_slice(&self.version.patch.to_le_bytes());
        out.extend_from_slice(&self.apk_sha256);
        out.extend_from_slice(&self.timestamp.to_le_bytes());
        out
    }

    /// Signer avec la clé racine de l'équipe. Retourne (signature, octets signés).
    pub fn sign(&self, root: &Identity) -> ([u8; 64], Vec<u8>) {
        let bytes = self.to_signed_bytes();
        let sig = root.sign(&bytes);
        (sig, bytes)
    }

    /// Vérifier la signature contre la racine épinglée et parser l'annonce.
    pub fn verify(
        root_pubkey_bytes: &[u8; 32],
        data: &[u8],
        sig: &[u8; 64],
    ) -> Result<Self, String> {
        if data.len() != 8 + 6 + 32 + 8 || &data[..8] != ANNOUNCEMENT_MAGIC {
            return Err("Malformed update announcement".to_string());
        }
        if !Identity::verify_from_pubkey(root_pubkey_bytes, data, sig) {
            return Err("Update announcement signature invalid or untrusted".to_string());
        }
        let rd_u16 = |off: usize| u16::from_le_bytes([data[off], data[off + 1]]);
        let mut apk_sha256 = [0u8; 32];
        apk_sha256.copy_from_slice(&data[8 + 6..8 + 6 + 32]);
        Ok(Self {
            version: Version::new(rd_u16(8), rd_u16(10), rd_u16(12)),
            apk_sha256,
            timestamp: u64::from_le_bytes(
                data[8 + 6 + 32..8 + 6 + 32 + 8]
                    .try_into()
                    .map_err(|_| "Malformed announcement timestamp".to_string())?,
            ),
        })
    }

    /// Hash d'un fichier APK (SHA-256).
    pub fn hash_apk(apk: &[u8]) -> [u8; 32] {
        Sha256::digest(apk).into()
    }
}

/// Manifeste de mise à jour : manifeste APK canonique signé par la racine,
/// accompagné des métadonnées de transfert (taille, chunk) non signées.
///
/// Le message sur le fil fait [`MANIFEST_WIRE_LEN`] octets :
/// `ApkManifest.sign() (80) || apk_size(8) || chunk_size(4)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// Manifeste canonique signé (magic `ONDEAPK1`, hash, clé dev, build).
    pub apk_manifest: ApkManifest,
    /// Taille de l'APK en octets (métadonnée non signée, liée au hash signé).
    pub apk_size: u64,
    /// Taille de chunk demandée pour le transfert (métadonnée non signée).
    pub chunk_size: u32,
}

impl UpdateManifest {
    /// Construire le message manifeste complet (blob signé + métadonnées).
    pub fn to_wire_bytes(&self) -> Vec<u8> {
        let mut out = self.apk_manifest.to_signed_bytes();
        out.extend_from_slice(&self.apk_size.to_le_bytes());
        out.extend_from_slice(&self.chunk_size.to_le_bytes());
        out
    }

    /// Signer avec la clé racine. Retourne (signature, message complet).
    pub fn sign(&self, root: &Identity) -> ([u8; 64], Vec<u8>) {
        let (sig, _) = self.apk_manifest.sign(root);
        (sig, self.to_wire_bytes())
    }

    /// Vérifier la signature racine du manifeste canonique et parser le
    /// message complet (sans l'APK — la vérification du hash se fait lors de
    /// l'assemblage via [`crate::crypto::verify_apk_signature`]).
    pub fn verify(
        root_pubkey_bytes: &[u8; 32],
        data: &[u8],
        sig: &[u8; 64],
    ) -> Result<Self, String> {
        if data.len() != MANIFEST_WIRE_LEN {
            return Err("Malformed update manifest".to_string());
        }
        if !ApkManifest::verify(root_pubkey_bytes, &data[..SIGNED_MANIFEST_LEN], sig) {
            return Err("Update manifest signature invalid or untrusted".to_string());
        }
        let apk_manifest = parse_signed_manifest(&data[..SIGNED_MANIFEST_LEN])?;
        let apk_size =
            u64::from_le_bytes(data[SIGNED_MANIFEST_LEN..SIGNED_MANIFEST_LEN + 8].try_into().unwrap());
        let chunk_size = u32::from_le_bytes(
            data[SIGNED_MANIFEST_LEN + 8..SIGNED_MANIFEST_LEN + 8 + 4]
                .try_into()
                .unwrap(),
        );
        Ok(Self {
            apk_manifest,
            apk_size,
            chunk_size,
        })
    }
}

/// Parser le manifeste canonique signé (80 octets, magic `ONDEAPK1`).
fn parse_signed_manifest(data: &[u8]) -> Result<ApkManifest, String> {
    if data.len() != SIGNED_MANIFEST_LEN || &data[..8] != crate::crypto::APK_MAGIC {
        return Err("Malformed signed manifest".to_string());
    }
    let mut apk_hash = [0u8; 32];
    apk_hash.copy_from_slice(&data[8..40]);
    let mut dev_pubkey = [0u8; 32];
    dev_pubkey.copy_from_slice(&data[40..72]);
    let timestamp = u64::from_le_bytes(
        data[72..80]
            .try_into()
            .map_err(|_| "Malformed signed manifest timestamp".to_string())?,
    );
    Ok(ApkManifest {
        apk_hash,
        dev_pubkey,
        timestamp,
    })
}

/// Version installée (enregistrée après vérification complète).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledUpdate {
    pub version: Version,
    pub apk_sha256: [u8; 32],
    pub installed_at: u64,
    pub apk_path: Option<String>,
}

/// Statut d'un chunk reçu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkStatus {
    /// Chunk valide et enregistré.
    Accepted,
    /// Chunk en double (ignoré, pas une erreur).
    Duplicate,
}

/// Erreurs du protocole de mise à jour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// Annonce/manifeste malformé ou mal signé.
    InvalidSignature(String),
    /// Version non supérieure à la version courante (downgrade ou égale).
    NotNewer { current: Version, offered: Version },
    /// Le manifeste ne correspond pas à l'annonce (version ou hash différent).
    ManifestMismatch,
    /// Aucun manifeste validé avant de recevoir des chunks.
    NoPendingTransfer,
    /// Index de chunk hors bornes.
    ChunkIndexOutOfBounds { index: u32, expected: usize },
    /// Taille de chunk invalide (dépasse la taille annoncée).
    ChunkTooLarge { index: u32, len: usize, max: usize },
    /// Fichier incomplet ou trop volumineux.
    IncompleteTransfer { received: usize, expected: u64 },
    /// L'APK assemblé a échoué la vérification de bout en bout.
    VerificationFailed(String),
    /// La cible d'installation est invalide.
    InstallFailed(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSignature(m) => write!(f, "invalid signature: {m}"),
            Self::NotNewer { current, offered } => {
                write!(f, "offered version {offered} is not newer than current {current}")
            }
            Self::ManifestMismatch => write!(f, "manifest does not match announcement"),
            Self::NoPendingTransfer => write!(f, "no validated manifest before chunks"),
            Self::ChunkIndexOutOfBounds { index, expected } => {
                write!(f, "chunk {index} out of bounds (expected {expected} chunks)")
            }
            Self::ChunkTooLarge { index, len, max } => {
                write!(f, "chunk {index} too large ({len} bytes, max {max})")
            }
            Self::IncompleteTransfer { received, expected } => {
                write!(f, "incomplete transfer: {received} bytes received, expected {expected}")
            }
            Self::VerificationFailed(m) => write!(f, "APK verification failed: {m}"),
            Self::InstallFailed(m) => write!(f, "install failed: {m}"),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<String> for UpdateError {
    fn from(s: String) -> Self {
        Self::InvalidSignature(s)
    }
}

/// État en cours de téléchargement côté receveur.
#[derive(Debug)]
struct PendingUpdate {
    version: Version,
    manifest: UpdateManifest,
    manifest_wire: Vec<u8>,
    manifest_signature: [u8; 64],
    peer: String,
    chunks: HashMap<u32, Vec<u8>>,
    expected_chunks: usize,
}

/// APK qui a passé la vérification de bout en bout, en attente d'installation.
#[derive(Debug, Clone)]
struct VerifiedState {
    version: Version,
    apk_sha256: [u8; 32],
}

/// Machine à états du protocole de mise à jour.
#[derive(Debug)]
pub struct UpdateProtocol {
    root_pubkey: [u8; 32],
    current_version: Version,
    pending: Option<PendingUpdate>,
    last_verified: Option<VerifiedState>,
    installed: Vec<InstalledUpdate>,
}

impl UpdateProtocol {
    /// Créer une machine à états avec la racine de confiance épinglée.
    pub fn new(root_pubkey: [u8; 32], current_version: Version) -> Self {
        Self {
            root_pubkey,
            current_version,
            pending: None,
            last_verified: None,
            installed: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Côté annonceur
    // ------------------------------------------------------------------

    /// Construire et signer l'annonce d'une nouvelle version.
    pub fn build_announcement(
        version: Version,
        apk: &[u8],
        root: &Identity,
        timestamp: u64,
    ) -> (UpdateAnnouncement, [u8; 64], Vec<u8>) {
        let announcement = UpdateAnnouncement {
            version,
            apk_sha256: UpdateAnnouncement::hash_apk(apk),
            timestamp,
        };
        let (sig, bytes) = announcement.sign(root);
        (announcement, sig, bytes)
    }

    /// Construire et signer le manifeste de mise à jour.
    ///
    /// La version est portée par l'annonce signée ; le manifeste transporte
    /// le hash, la clé de build et l'horodatage.
    ///
    /// `dev_pubkey` est la clé de la chaîne de build qui a signé l'APK
    /// (enregistrée dans le manifeste canonique).
    pub fn build_manifest(
        apk: &[u8],
        root: &Identity,
        dev_pubkey: [u8; 32],
        timestamp: u64,
        chunk_size: u32,
    ) -> (UpdateManifest, [u8; 64], Vec<u8>) {
        let apk_manifest = ApkManifest::from_apk(apk, dev_pubkey, timestamp);
        let manifest = UpdateManifest {
            apk_manifest,
            apk_size: apk.len() as u64,
            chunk_size,
        };
        let (sig, wire) = manifest.sign(root);
        (manifest, sig, wire)
    }

    /// Nombre de chunks nécessaires pour transférer `apk_len` octets.
    pub fn chunk_count(apk_len: usize, chunk_size: usize) -> usize {
        if chunk_size == 0 {
            return 0;
        }
        apk_len.div_ceil(chunk_size)
    }

    /// Extraire le chunk `index` d'un APK (None si hors bornes).
    pub fn chunk(apk: &[u8], index: u32, chunk_size: usize) -> Option<Vec<u8>> {
        let start = index as usize * chunk_size;
        if start >= apk.len() {
            return None;
        }
        let end = (start + chunk_size).min(apk.len());
        Some(apk[start..end].to_vec())
    }

    // ------------------------------------------------------------------
    // Côté receveur — étape 1 : annonce
    // ------------------------------------------------------------------

    /// Traiter une annonce reçue. Vérifie la signature racine et exige que la
    /// version soit strictement supérieure à la version courante.
    pub fn handle_announcement(
        &self,
        data: &[u8],
        sig: &[u8; 64],
    ) -> Result<UpdateAnnouncement, UpdateError> {
        let announcement = UpdateAnnouncement::verify(&self.root_pubkey, data, sig)?;
        if announcement.version <= self.current_version {
            return Err(UpdateError::NotNewer {
                current: self.current_version,
                offered: announcement.version,
            });
        }
        Ok(announcement)
    }

    // ------------------------------------------------------------------
    // Côté receveur — étape 2 : manifeste
    // ------------------------------------------------------------------

    /// Traiter le manifeste reçu. Le manifeste canonique doit correspondre à
    /// l'annonce (même SHA-256) et être signé par la racine. Initialise le
    /// transfert en cours.
    pub fn handle_manifest(
        &mut self,
        announcement: &UpdateAnnouncement,
        data: &[u8],
        sig: &[u8; 64],
        peer: &str,
    ) -> Result<(), UpdateError> {
        let manifest = UpdateManifest::verify(&self.root_pubkey, data, sig)?;
        // Liaison annonce ↔ manifeste : même APK (hash signé identique).
        if manifest.apk_manifest.apk_hash != announcement.apk_sha256 {
            return Err(UpdateError::ManifestMismatch);
        }
        if manifest.apk_size > MAX_APK_SIZE || manifest.chunk_size == 0 {
            return Err(UpdateError::InvalidSignature(
                "manifest size or chunk size out of bounds".to_string(),
            ));
        }
        let expected_chunks =
            Self::chunk_count(manifest.apk_size as usize, manifest.chunk_size as usize);
        if expected_chunks > MAX_CHUNKS {
            return Err(UpdateError::InvalidSignature(format!(
                "too many chunks ({expected_chunks} > {MAX_CHUNKS})"
            )));
        }
        self.pending = Some(PendingUpdate {
            version: announcement.version,
            manifest,
            manifest_wire: data.to_vec(),
            manifest_signature: *sig,
            peer: peer.to_string(),
            chunks: HashMap::with_capacity(expected_chunks.min(1024)),
            expected_chunks,
        });
        // Un nouveau transfert invalide toute vérification précédente.
        self.last_verified = None;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Côté receveur — étape 3 : chunks
    // ------------------------------------------------------------------

    /// Traiter un chunk APK reçu. Valide l'index et la taille, puis l'enregistre.
    pub fn handle_chunk(&mut self, index: u32, data: &[u8]) -> Result<ChunkStatus, UpdateError> {
        let pending = self
            .pending
            .as_mut()
            .ok_or(UpdateError::NoPendingTransfer)?;
        if index as usize >= pending.expected_chunks {
            return Err(UpdateError::ChunkIndexOutOfBounds {
                index,
                expected: pending.expected_chunks,
            });
        }
        let chunk_size = pending.manifest.chunk_size as usize;
        let max_len = if index as usize == pending.expected_chunks - 1 {
            (pending.manifest.apk_size as usize).saturating_sub(index as usize * chunk_size)
        } else {
            chunk_size
        };
        if data.len() > max_len {
            return Err(UpdateError::ChunkTooLarge {
                index,
                len: data.len(),
                max: max_len,
            });
        }
        if pending.chunks.contains_key(&index) {
            return Ok(ChunkStatus::Duplicate);
        }
        pending.chunks.insert(index, data.to_vec());
        Ok(ChunkStatus::Accepted)
    }

    /// Nombre de chunks reçus et valides.
    pub fn chunks_received(&self) -> usize {
        self.pending
            .as_ref()
            .map(|p| p.chunks.len())
            .unwrap_or(0)
    }

    /// Nombre total de chunks attendus (0 si aucun transfert en cours).
    pub fn total_chunks(&self) -> usize {
        self.pending.as_ref().map(|p| p.expected_chunks).unwrap_or(0)
    }

    /// L'identifiant du pair qui fournit le transfert en cours.
    pub fn pending_peer(&self) -> Option<&str> {
        self.pending.as_ref().map(|p| p.peer.as_str())
    }

    // ------------------------------------------------------------------
    // Côté receveur — étape 4 : assemblage + vérification de bout en bout
    // ------------------------------------------------------------------

    /// Assembler les chunks, vérifier la taille totale, puis exécuter la
    /// vérification complète de l'APK ([`crate::crypto::verify_apk_signature`] :
    /// signature Ed25519 du manifeste contre la racine épinglée + SHA-256 de
    /// l'APK entier).
    ///
    /// Retourne l'APK vérifié. En cas de vérification échouée (APK falsifié),
    /// le transfert en cours est purgé — un transfert empoisonné ne peut pas
    /// être repris. En cas de transfert simplement incomplet, l'état est
    /// conservé pour permettre la reprise.
    pub fn assemble_and_verify(&mut self) -> Result<Vec<u8>, UpdateError> {
        // Lecture de l'état sans consommation (pour permettre la reprise sur
        // transfert incomplet).
        let (version, manifest_wire, manifest_signature, expected_chunks, apk_size) =
            match &self.pending {
                Some(p) => (
                    p.version,
                    p.manifest_wire.clone(),
                    p.manifest_signature,
                    p.expected_chunks,
                    p.manifest.apk_size,
                ),
                None => return Err(UpdateError::NoPendingTransfer),
            };
        if self.pending.as_ref().unwrap().chunks.len() != expected_chunks {
            return Err(UpdateError::IncompleteTransfer {
                received: self.pending.as_ref().unwrap().chunks.len(),
                expected: expected_chunks as u64,
            });
        }
        let mut apk = Vec::with_capacity(apk_size as usize);
        for i in 0..expected_chunks {
            let chunk = self
                .pending
                .as_ref()
                .unwrap()
                .chunks
                .get(&(i as u32))
                .ok_or(UpdateError::IncompleteTransfer {
                    received: self.pending.as_ref().unwrap().chunks.len(),
                    expected: expected_chunks as u64,
                })?;
            apk.extend_from_slice(chunk);
        }
        if apk.len() as u64 != apk_size {
            return Err(UpdateError::IncompleteTransfer {
                received: apk.len(),
                expected: apk_size,
            });
        }
        // À partir d'ici le transfert est consommé, que la vérification
        // réussisse ou échoue : un APK falsifié ne doit pas pouvoir être
        // ré-essayé à l'infini avec le même manifeste.
        self.pending = None;
        verify_apk_signature(
            &apk,
            &manifest_wire[..SIGNED_MANIFEST_LEN],
            &manifest_signature,
            &self.root_pubkey,
        )
        .map_err(UpdateError::VerificationFailed)?;
        self.last_verified = Some(VerifiedState {
            version,
            apk_sha256: UpdateAnnouncement::hash_apk(&apk),
        });
        Ok(apk)
    }

    // ------------------------------------------------------------------
    // Côté receveur — étape 5 : installation (point d'intégration)
    // ------------------------------------------------------------------

    /// Écrire l'APK vérifié sur disque (simulation d'installation utilisée par
    /// les tests et le mode desktop). Retourne la version installée.
    ///
    /// Seul l'APK qui a passé [`UpdateProtocol::assemble_and_verify`] (hash
    /// exact) peut être installé, et une seule fois : l'état vérifié est
    /// consommé par l'installation.
    pub fn install_verified(
        &mut self,
        apk: &[u8],
        dest_path: &str,
        installed_at: u64,
    ) -> Result<InstalledUpdate, UpdateError> {
        let last = self
            .last_verified
            .clone()
            .ok_or(UpdateError::NoPendingTransfer)?;
        // L'APK fourni doit être exactement celui qui a passé la vérification
        // de bout en bout (même SHA-256) : aucun APK non vérifié ne peut
        // atterrir sur disque.
        if UpdateAnnouncement::hash_apk(apk) != last.apk_sha256 {
            return Err(UpdateError::VerificationFailed(
                "APK content hash mismatch (tampered APK)".to_string(),
            ));
        }
        std::fs::write(dest_path, apk).map_err(|e| UpdateError::InstallFailed(e.to_string()))?;
        let installed = InstalledUpdate {
            version: last.version,
            apk_sha256: last.apk_sha256,
            installed_at,
            apk_path: Some(dest_path.to_string()),
        };
        self.installed.push(installed.clone());
        self.installed.sort_by_key(|u| u.version);
        self.current_version = self
            .installed
            .last()
            .map(|u| u.version)
            .unwrap_or(self.current_version);
        // L'état vérifié est consommé : une seule installation par vérification.
        self.last_verified = None;
        self.pending = None;
        Ok(installed)
    }

    /// Enregistrer une version installée sans écrire de fichier (pour les
    /// plateformes où l'installation est déléguée à PackageInstaller).
    pub fn record_install(
        &mut self,
        version: Version,
        apk_sha256: [u8; 32],
        installed_at: u64,
        apk_path: Option<String>,
    ) {
        self.installed.push(InstalledUpdate {
            version,
            apk_sha256,
            installed_at,
            apk_path,
        });
        self.installed.sort_by_key(|u| u.version);
        self.current_version = self
            .installed
            .last()
            .map(|u| u.version)
            .unwrap_or(self.current_version);
    }

    /// Purger le transfert en cours et toute vérification en attente
    /// (rejet d'un APK invalide, timeout, abandon).
    pub fn reject_pending(&mut self) {
        self.pending = None;
        self.last_verified = None;
    }

    // ------------------------------------------------------------------
    // État
    // ------------------------------------------------------------------

    pub fn root_pubkey(&self) -> &[u8; 32] {
        &self.root_pubkey
    }

    pub fn current_version(&self) -> Version {
        self.current_version
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Un APK a passé la vérification de bout en bout et attend l'installation.
    pub fn has_verified(&self) -> bool {
        self.last_verified.is_some()
    }

    pub fn installed_updates(&self) -> &[InstalledUpdate] {
        &self.installed
    }

    pub fn latest_installed(&self) -> Option<&InstalledUpdate> {
        self.installed.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Identity;

    /// Faux APK déterministe (simule un fichier de ~40 Kio).
    fn fake_apk(size: usize) -> Vec<u8> {
        (0..size).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn version_parse_and_ordering() {
        let v = Version::parse("2.3.4").unwrap();
        assert_eq!(v, Version::new(2, 3, 4));
        assert!(Version::new(2, 3, 4) > Version::new(2, 3, 3));
        assert!(Version::new(2, 4, 0) > Version::new(2, 3, 99));
        assert!(Version::new(3, 0, 0) > Version::new(2, 99, 99));
        assert_eq!(v.to_string(), "2.3.4");
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
        assert!(Version::parse("1.x.3").is_err());
    }

    #[test]
    fn chunking_math() {
        let apk = fake_apk(40_000);
        let cs = DEFAULT_CHUNK_SIZE as usize;
        let n = UpdateProtocol::chunk_count(apk.len(), cs);
        assert_eq!(n, 3);
        let rebuilt: Vec<u8> = (0..n as u32)
            .flat_map(|i| UpdateProtocol::chunk(&apk, i, cs).unwrap())
            .collect();
        assert_eq!(rebuilt, apk);
        assert!(UpdateProtocol::chunk(&apk, n as u32, cs).is_none());
    }

    #[test]
    fn announcement_sign_and_verify() {
        let root = Identity::generate();
        let apk = fake_apk(1024);
        let (ann, sig, bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk, &root, 1000);
        assert_eq!(ann.apk_sha256, UpdateAnnouncement::hash_apk(&apk));
        let parsed = UpdateAnnouncement::verify(&root.verifying_key_bytes(), &bytes, &sig).unwrap();
        assert_eq!(parsed, ann);

        // Signature invalide → rejet
        let bad_sig = [0u8; 64];
        assert!(UpdateAnnouncement::verify(&root.verifying_key_bytes(), &bytes, &bad_sig).is_err());
        // Mauvaise racine → rejet
        let other = Identity::generate();
        assert!(UpdateAnnouncement::verify(&other.verifying_key_bytes(), &bytes, &sig).is_err());
        // Format malformé → rejet
        assert!(UpdateAnnouncement::verify(&root.verifying_key_bytes(), b"short", &sig).is_err());
    }

    #[test]
    fn manifest_sign_and_verify() {
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk = fake_apk(2048);
        let (man, sig, bytes) =
            UpdateProtocol::build_manifest(&apk, &root, dev.verifying_key_bytes(), 2000, 1024);
        assert_eq!(man.apk_size, 2048);
        assert_eq!(man.apk_manifest.apk_hash, UpdateAnnouncement::hash_apk(&apk));
        let parsed = UpdateManifest::verify(&root.verifying_key_bytes(), &bytes, &sig).unwrap();
        assert_eq!(parsed, man);
        // Mauvaise racine → rejet
        let other = Identity::generate();
        assert!(UpdateManifest::verify(&other.verifying_key_bytes(), &bytes, &sig).is_err());
        // Longueur tronquée → rejet
        assert!(UpdateManifest::verify(&root.verifying_key_bytes(), &bytes[..80], &sig).is_err());
    }

    #[test]
    fn full_flow_announce_manifest_chunks_verify_install() {
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk = fake_apk(40_000);
        let new_version = Version::new(1, 0, 1);
        let mut receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(1, 0, 0));

        // 1. Annonce
        let (ann, ann_sig, ann_bytes) =
            UpdateProtocol::build_announcement(new_version, &apk, &root, 3000);
        let parsed = receiver.handle_announcement(&ann_bytes, &ann_sig).unwrap();
        assert_eq!(parsed, ann);

        // 2. Manifeste
        let (_man, man_sig, man_bytes) = UpdateProtocol::build_manifest(
            &apk,
            &root,
            dev.verifying_key_bytes(),
            3001,
            DEFAULT_CHUNK_SIZE,
        );
        receiver
            .handle_manifest(&ann, &man_bytes, &man_sig, "peer-alice")
            .unwrap();
        assert_eq!(receiver.total_chunks(), 3);
        assert_eq!(receiver.pending_peer(), Some("peer-alice"));

        // 3. Chunks (avec un doublon pour tester l'idempotence)
        assert_eq!(
            receiver
                .handle_chunk(0, &UpdateProtocol::chunk(&apk, 0, DEFAULT_CHUNK_SIZE as usize).unwrap())
                .unwrap(),
            ChunkStatus::Accepted
        );
        assert_eq!(
            receiver
                .handle_chunk(0, &UpdateProtocol::chunk(&apk, 0, DEFAULT_CHUNK_SIZE as usize).unwrap())
                .unwrap(),
            ChunkStatus::Duplicate
        );
        for i in 1..3 {
            let chunk = UpdateProtocol::chunk(&apk, i, DEFAULT_CHUNK_SIZE as usize).unwrap();
            receiver.handle_chunk(i, &chunk).unwrap();
        }
        assert_eq!(receiver.chunks_received(), 3);

        // 4. Assemblage + vérification
        let verified_apk = receiver.assemble_and_verify().unwrap();
        assert_eq!(verified_apk, apk);

        // 5. Installation
        let dest = std::env::temp_dir().join("onde-update-test-install.apk");
        let installed = receiver
            .install_verified(&verified_apk, dest.to_str().unwrap(), 4000)
            .unwrap();
        assert_eq!(installed.version, new_version);
        assert_eq!(installed.apk_sha256, UpdateAnnouncement::hash_apk(&apk));
        assert!(receiver.latest_installed().is_some());
        assert_eq!(receiver.current_version(), new_version);
        assert!(!receiver.has_pending());
        let _ = std::fs::remove_file(&dest);
    }

    #[test]
    fn reject_announcement_downgrade_or_equal() {
        let root = Identity::generate();
        let apk = fake_apk(1024);
        let receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(2, 0, 0));
        // Offre une version plus ancienne → rejet NotNewer
        let (_, sig, bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 0), &apk, &root, 5000);
        assert!(matches!(
            receiver.handle_announcement(&bytes, &sig),
            Err(UpdateError::NotNewer { .. })
        ));
        // Offre la version courante → rejet NotNewer
        let (_, sig, bytes) =
            UpdateProtocol::build_announcement(Version::new(2, 0, 0), &apk, &root, 5001);
        assert!(matches!(
            receiver.handle_announcement(&bytes, &sig),
            Err(UpdateError::NotNewer { .. })
        ));
        // Version supérieure → accepté
        let (_, sig, bytes) =
            UpdateProtocol::build_announcement(Version::new(2, 0, 1), &apk, &root, 5002);
        assert!(receiver.handle_announcement(&bytes, &sig).is_ok());
    }

    #[test]
    fn reject_manifest_mismatch() {
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk_a = fake_apk(4096);
        let apk_b = fake_apk(4097); // APK différent du contenu annoncé
        let mut receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(1, 0, 0));

        let (ann, ann_sig, ann_bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk_a, &root, 6000);
        receiver.handle_announcement(&ann_bytes, &ann_sig).unwrap();

        // Manifeste annonçant un hash différent (apk_b) → rejet ManifestMismatch
        let (man, man_sig, man_bytes) = UpdateProtocol::build_manifest(
            &apk_b,
            &root,
            dev.verifying_key_bytes(),
            6001,
            1024,
        );
        assert!(matches!(
            receiver.handle_manifest(&ann, &man_bytes, &man_sig, "peer"),
            Err(UpdateError::ManifestMismatch)
        ));
        let _ = man;
    }

    #[test]
    fn reject_tampered_apk() {
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk = fake_apk(40_000);
        let mut receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(1, 0, 0));

        // Signature invalide → rejet dès l'annonce
        let (_ann, _, ann_bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk, &root, 7000);
        assert!(matches!(
            receiver.handle_announcement(&ann_bytes, &[0u8; 64]),
            Err(UpdateError::InvalidSignature(_))
        ));

        // Flux valide jusqu'à l'assemblage
        let (_ann, ann_sig, ann_bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk, &root, 7000);
        let parsed_ann = receiver.handle_announcement(&ann_bytes, &ann_sig).unwrap();
        let (_, man_sig, man_bytes) = UpdateProtocol::build_manifest(
            &apk,
            &root,
            dev.verifying_key_bytes(),
            7001,
            DEFAULT_CHUNK_SIZE,
        );
        receiver
            .handle_manifest(&parsed_ann, &man_bytes, &man_sig, "peer")
            .unwrap();

        // Falsifier un octet du chunk 1
        let mut evil_chunk = UpdateProtocol::chunk(&apk, 1, DEFAULT_CHUNK_SIZE as usize).unwrap();
        evil_chunk[0] ^= 0xFF;
        receiver
            .handle_chunk(0, &UpdateProtocol::chunk(&apk, 0, DEFAULT_CHUNK_SIZE as usize).unwrap())
            .unwrap();
        receiver.handle_chunk(1, &evil_chunk).unwrap();
        receiver
            .handle_chunk(2, &UpdateProtocol::chunk(&apk, 2, DEFAULT_CHUNK_SIZE as usize).unwrap())
            .unwrap();

        assert!(matches!(
            receiver.assemble_and_verify(),
            Err(UpdateError::VerificationFailed(_))
        ));
        // Le transfert est purgé après l'échec
        assert!(!receiver.has_pending());
    }

    #[test]
    fn reject_chunk_out_of_bounds_and_incomplete() {
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk = fake_apk(4096);
        let mut receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(1, 0, 0));

        // Chunk avant tout manifeste → NoPendingTransfer
        assert!(matches!(
            receiver.handle_chunk(0, b"data"),
            Err(UpdateError::NoPendingTransfer)
        ));

        let (_ann, ann_sig, ann_bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk, &root, 8000);
        let ann = receiver.handle_announcement(&ann_bytes, &ann_sig).unwrap();
        let (_, man_sig, man_bytes) = UpdateProtocol::build_manifest(
            &apk,
            &root,
            dev.verifying_key_bytes(),
            8001,
            1024,
        );
        receiver.handle_manifest(&ann, &man_bytes, &man_sig, "peer").unwrap();

        // Index hors bornes
        assert!(matches!(
            receiver.handle_chunk(9, b"data"),
            Err(UpdateError::ChunkIndexOutOfBounds { .. })
        ));
        // Taille de chunk excessive (dernier chunk plus grand que le reste)
        assert!(matches!(
            receiver.handle_chunk(3, &[0u8; 4096]),
            Err(UpdateError::ChunkTooLarge { .. })
        ));
        // Transfert incomplet → IncompleteTransfer
        receiver
            .handle_chunk(0, &UpdateProtocol::chunk(&apk, 0, 1024).unwrap())
            .unwrap();
        assert!(matches!(
            receiver.assemble_and_verify(),
            Err(UpdateError::IncompleteTransfer { .. })
        ));
    }

    #[test]
    fn full_flow_rejects_apk_signed_by_unknown_root() {
        // Même flux complet, mais l'annonce signée par une racine qui n'est PAS
        // la racine épinglée du receveur → rejet dès l'annonce.
        let unknown_root = Identity::generate();
        let root = Identity::generate();
        let apk = fake_apk(2048);
        let receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(1, 0, 0));

        let (_, sig, bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk, &unknown_root, 9000);
        assert!(matches!(
            receiver.handle_announcement(&bytes, &sig),
            Err(UpdateError::InvalidSignature(_))
        ));
    }

    #[test]
    fn manifest_unsigned_metadata_does_not_bypass_verification() {
        // Les métadonnées non signées (apk_size/chunk_size) ne peuvent pas
        // faire accepter un APK falsifié : le hash signé reste la seule porte
        // d'entrée vers l'installation.
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk = fake_apk(4096);
        let mut receiver = UpdateProtocol::new(root.verifying_key_bytes(), Version::new(1, 0, 0));

        let (_ann, ann_sig, ann_bytes) =
            UpdateProtocol::build_announcement(Version::new(1, 0, 1), &apk, &root, 10000);
        let ann = receiver.handle_announcement(&ann_bytes, &ann_sig).unwrap();

        // Construire un manifeste dont le blob signé est légitime mais dont la
        // taille annoncée (non signée) est fausse → le transfert devient
        // incohérent et l'assemblage échoue (jamais d'installation).
        let (_man, man_sig, mut man_bytes) = UpdateProtocol::build_manifest(
            &apk,
            &root,
            dev.verifying_key_bytes(),
            10001,
            1024,
        );
        // Corrompre la métadonnée apk_size (décalage 80..88) sans toucher au
        // blob signé : la signature reste valide (elle ne porte que sur les
        // 80 premiers octets), donc le manifeste corrompu est accepté — mais
        // la taille annoncée ne correspond à aucune APK transférable, et le
        // flux d'assemblage/vérification échoue avant toute installation.
        man_bytes[80..88].copy_from_slice(&999_999u64.to_le_bytes());
        // Soit le manifeste est rejeté ici, soit il est accepté mais le
        // transfert ne peut jamais être complet → les deux cas sont sûrs.
        let res = receiver.handle_manifest(&ann, &man_bytes, &man_sig, "peer");
        if res.is_ok() {
            receiver.handle_chunk(0, &UpdateProtocol::chunk(&apk, 0, 1024).unwrap()).unwrap();
            // 4096 octets annoncés en 1024 → 4 chunks attendus, mais le pair
            // n'a qu'un seul chunk : transfert incomplet → rejet.
            assert!(matches!(
                receiver.assemble_and_verify(),
                Err(UpdateError::IncompleteTransfer { .. })
            ));
        }
        assert!(receiver.latest_installed().is_none());
    }
}