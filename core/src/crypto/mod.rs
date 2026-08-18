/// Cryptography — Ed25519 identities, X25519 ECDH encryption, ZK transactions
use ed25519_dalek::{SigningKey, VerifyingKey, Verifier, Signer};
use ed25519_dalek::Signature as EdSignature;
use chacha20poly1305::{
    aead::AeadInPlace,
    ChaCha20Poly1305, Key, Nonce, KeyInit,
};
use rand::rngs::OsRng as RandOsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret as X25519StaticSecret};
use zeroize::Zeroize;

/// Cryptographic identity for ONDE nodes.
///
/// Each identity carries two keypairs:
/// - Ed25519 — signatures / verification (identities, event signing),
/// - X25519 — ECDH key exchange for end-to-end message encryption.
///
/// ⚠️ **Couplage de clés assumé (Audit m8)** : la clé X25519 est dérivée
/// **déterministiquement** de la seed Ed25519 (`derive_x25519`). Conséquence
/// directe : la compromission de la clé de signature Ed25519 entraîne la
/// compromission de la clé de chiffrement X25519 (même secret racine). Ce
/// couplage est un choix **assumé** : il permet de restaurer l'identité
/// complète (signature + chiffrement) depuis un unique secret de 32 octets
/// (`from_bytes`), au prix d'une séparation des domaines moindre qu'une
/// génération indépendante des deux paires. Ne pas dériver d'autres secrets
/// de cette seed sans reconsidérer ce compromis.
#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    x25519_secret: X25519StaticSecret,
    x25519_public: X25519PublicKey,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // N'affiche jamais les clés secrètes — seulement les clés publiques.
        f.debug_struct("Identity")
            .field("verifying_key_hex", &self.pubkey_hex())
            .field("x25519_public_key_hex", &self.x25519_public_key_hex())
            .finish_non_exhaustive()
    }
}

impl Identity {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut RandOsRng);
        let verifying_key = signing_key.verifying_key();
        // Derive the X25519 ECDH key deterministically from the Ed25519 seed
        // so `from_bytes` (restore) reproduces the exact same keypair.
        let (x25519_secret, x25519_public) = Self::derive_x25519(signing_key.as_bytes());
        Self {
            signing_key,
            verifying_key,
            x25519_secret,
            x25519_public,
        }
    }

    /// Restore an identity from the 32-byte Ed25519 seed.
    ///
    /// The X25519 ECDH key is derived DETERMINISTICALLY from the Ed25519
    /// signing key (HKDF-SHA256), so every node that holds the same secret
    /// reproduces the exact same X25519 keypair. Peers that cached the public
    /// X25519 key (for encrypted replies) keep working across restarts.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(bytes);
        let verifying_key = signing_key.verifying_key();
        let (x25519_secret, x25519_public) = Self::derive_x25519(signing_key.as_bytes());
        Self {
            signing_key,
            verifying_key,
            x25519_secret,
            x25519_public,
        }
    }

    /// Derive a stable X25519 keypair from an Ed25519 secret key
    /// (HKDF-SHA256, info = "onde-x25519-v1"). Deterministic for a given seed.
    ///
    /// ⚠️ **Implication sécurité (Audit m8)** : ce couplage signifie que la
    /// compromission de la clé Ed25519 (signature) compromet aussi la clé
    /// X25519 (chiffrement). C'est le compromis assumé pour la restauration
    /// déterministe de l'identité depuis un unique seed.
    fn derive_x25519(seed: &[u8]) -> (X25519StaticSecret, X25519PublicKey) {
        let hk = hkdf::Hkdf::<Sha256>::new(None, seed);
        let mut x25519_material = [0u8; 32];
        hk.expand(b"onde-x25519-v1", &mut x25519_material)
            .expect("HKDF expand of a 32-byte seed cannot fail");
        // Clamp the derived scalar to a valid X25519 secret key
        let clamped: [u8; 32] = {
            let mut m = x25519_material;
            m[0] &= 0xF8;
            m[31] = (m[31] & 0x7F) | 0x40;
            m
        };
        let x25519_secret = X25519StaticSecret::from(clamped);
        let x25519_public = X25519PublicKey::from(&x25519_secret);
        // Zéroïse le matériel de dérivation intermédiaire (Audit m6)
        x25519_material.zeroize();
        (x25519_secret, x25519_public)
    }

    pub fn signing_key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }

    /// Get Ed25519 public key as hex
    pub fn pubkey_hex(&self) -> String {
        hex::encode(self.verifying_key_bytes())
    }

    /// X25519 public key bytes (used by peers to encrypt messages to us)
    pub fn x25519_public_key_bytes(&self) -> [u8; 32] {
        self.x25519_public.to_bytes()
    }

    /// X25519 public key as hex
    pub fn x25519_public_key_hex(&self) -> String {
        hex::encode(self.x25519_public_key_bytes())
    }

    /// Accessor to the X25519 static secret key (used by decryption)
    pub fn x25519_secret_key(&self) -> &X25519StaticSecret {
        &self.x25519_secret
    }

    /// X25519 secret key bytes
    pub fn x25519_secret_key_bytes(&self) -> [u8; 32] {
        self.x25519_secret.to_bytes()
    }

    /// Sign data (Ed25519)
    pub fn sign(&self, data: &[u8]) -> [u8; 64] {
        self.signing_key.sign(data).to_bytes()
    }

    /// Verify a signature (Ed25519)
    pub fn verify(&self, data: &[u8], sig_bytes: &[u8; 64]) -> bool {
        let sig = EdSignature::from_bytes(sig_bytes);
        self.verifying_key.verify(data, &sig).is_ok()
    }

    /// Verify signature from raw public key
    pub fn verify_from_pubkey(pubkey_bytes: &[u8; 32], data: &[u8], sig_bytes: &[u8; 64]) -> bool {
        if let Ok(vk) = VerifyingKey::from_bytes(pubkey_bytes) {
            let sig = EdSignature::from_bytes(sig_bytes);
            return vk.verify(data, &sig).is_ok();
        }
        false
    }
}

/// Encrypted message envelope — X25519 ECDH + HKDF-SHA256 + ChaCha20-Poly1305
/// (libsodium "box"-style sealed encryption).
///
/// Protocol (per message):
/// 1. Generate an ephemeral X25519 keypair.
/// 2. shared_secret = eph_secret * recipient_x25519_public (ECDH).
/// 3. key = HKDF-SHA256(ikm = shared_secret, salt = [0u8; 32], info = b"onde-mesh-v1", 32 bytes).
/// 4. Encrypt with ChaCha20-Poly1305 using a random 96-bit nonce.
///
/// The envelope stores the ephemeral public key, the nonce and the ciphertext,
/// so the recipient can re-derive the same key with their own X25519 secret key.
pub struct EncryptedEnvelope {
    /// ChaCha20-Poly1305 ciphertext
    pub ciphertext: Vec<u8>,
    /// 12-byte nonce
    pub nonce: [u8; 12],
    /// Sender X25519 public key (for reply)
    pub sender_pubkey: [u8; 32],
    /// Ephemeral X25519 public key used for the ECDH key exchange
    pub eph_public_key: [u8; 32],
}

impl EncryptedEnvelope {
    /// HKDF info string — binds derived keys to the ONDE mesh protocol
    const INFO: &'static [u8] = b"onde-mesh-v1";

    /// Derive the 32-byte ChaCha20-Poly1305 key from an X25519 shared secret
    fn derive_key(shared_secret: &[u8; 32]) -> Result<[u8; 32], String> {
        let hk = hkdf::Hkdf::<Sha256>::new(Some(&[0u8; 32]), shared_secret);
        let mut key = [0u8; 32];
        hk.expand(Self::INFO, &mut key).map_err(|e| e.to_string())?;
        Ok(key)
    }

    /// Encrypt a message for a recipient using the recipient's X25519 public key.
    ///
    /// The sender's X25519 public key is bound into the ciphertext via AEAD
    /// associated data, so a relay that replaces `sender_pubkey` is detected
    /// at decryption time (tag mismatch).
    pub fn encrypt(
        message: &[u8],
        sender: &Identity,
        recipient_pub_x25519: &[u8; 32],
    ) -> Result<Self, String> {
        let eph_secret = X25519StaticSecret::random_from_rng(RandOsRng);
        let eph_public = X25519PublicKey::from(&eph_secret);
        let recipient_public = X25519PublicKey::from(*recipient_pub_x25519);
        let shared_secret = eph_secret.diffie_hellman(&recipient_public);
        let key_bytes = Self::derive_key(shared_secret.as_bytes())?;
        let key = Key::from_slice(&key_bytes);
        let cipher = ChaCha20Poly1305::new(key);
        // Zéroïse le buffer de clé intermédiaire (Audit m6) : la clé n'existe
        // en clair que le temps de la création du chiffreur AEAD.
        let mut key_bytes = key_bytes;
        key_bytes.zeroize();

        let mut nonce_bytes = [0u8; 12];
        RandOsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let sender_pubkey = sender.x25519_public_key_bytes();
        let mut buf = message.to_vec();
        cipher
            .encrypt_in_place(nonce, &sender_pubkey, &mut buf)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            ciphertext: buf,
            nonce: nonce_bytes,
            sender_pubkey,
            eph_public_key: eph_public.to_bytes(),
        })
    }

    /// Decrypt an envelope as the recipient, using the recipient's X25519 secret key.
    ///
    /// `sender_pubkey` is part of the AEAD tag: a tampered or relay-substituted
    /// sender key causes an authentication failure.
    pub fn decrypt(envelope: &Self, recipient: &Identity) -> Result<Vec<u8>, String> {
        let eph_public = X25519PublicKey::from(envelope.eph_public_key);
        let shared_secret = recipient.x25519_secret_key().diffie_hellman(&eph_public);
        let key_bytes = Self::derive_key(shared_secret.as_bytes())?;
        let key = Key::from_slice(&key_bytes);
        let nonce = Nonce::from_slice(&envelope.nonce);
        let cipher = ChaCha20Poly1305::new(key);
        // Zéroïse le buffer de clé intermédiaire (Audit m6)
        let mut key_bytes = key_bytes;
        key_bytes.zeroize();

        let mut buf = envelope.ciphertext.to_vec();
        cipher
            .decrypt_in_place(nonce, &envelope.sender_pubkey, &mut buf)
            .map_err(|e| format!("Decryption failed (sender_pubkey tampered?): {e}"))?;
        Ok(buf)
    }
}

/*
 * Post-quantum / forward secrecy & identity hardening (Audit #10, #12, #14)
 */

/// Horodatage flou (±30 s) — anti-corrélation / anti-métadonnées (Audit #14).
///
/// Chaque nœud ajoute un décalage aléatoire uniforme dans [-30, +30] s à son
/// horodatage de création d'événement. La validation réseau tolère déjà
/// ±300 s (`MAX_CLOCK_SKEW`), donc l'ordre et la fraîcheur restent corrects
/// tout en brouillant le moment exact où un nœud a émis.
pub fn fuzzy_timestamp_secs() -> u64 {
    use rand::Rng;
    let skew: i64 = rand::thread_rng().gen_range(-30..=30);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    (now + skew).max(0) as u64
}

/// Identité à rotation automatique (Audit #10 — forward secrecy & rotation
/// d'identité).
///
/// La clé de signature reste la même à long terme (le mesh s'appuie sur les
/// identités stables pour la réputation), mais l'identité *publique* est
/// exposée avec un horodatage de rotation. Les messages récents sont signés
/// par `current` ; `previous` est conservé pour une période de grâce afin que
/// les pairs ayant mis en cache l'ancienne clé puissent continuer à vérifier.
/// À chaque rotation, les clés X25519 éphémères changent : un message chiffré
/// sous l'ancienne session ne peut pas être déchiffré avec la nouvelle clé
/// (forward secrecy renforcé).
#[derive(Debug, Clone)]
pub struct RotatingIdentity {
    current: Identity,
    previous: Option<Identity>,
    next: Identity,
    rotation_interval_secs: u64,
    last_rotation: u64,
    rotations: u64,
}

impl RotatingIdentity {
    /// Crée une identité rotative dont le compteur de rotation démarre
    /// à l'instant présent : la première rotation n'aura lieu qu'après
    /// `interval_secs`.
    pub fn new(interval_secs: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self::new_with_start(interval_secs, now)
    }

    /// Constructeur avec horodatage de démarrage explicite (tests).
    pub fn new_with_start(interval_secs: u64, last_rotation: u64) -> Self {
        Self {
            current: Identity::generate(),
            previous: None,
            next: Identity::generate(),
            rotation_interval_secs: interval_secs.max(60),
            last_rotation,
            rotations: 0,
        }
    }

    pub fn current(&self) -> &Identity {
        &self.current
    }

    pub fn current_pubkey_hex(&self) -> String {
        self.current.pubkey_hex()
    }

    pub fn rotation_count(&self) -> u64 {
        self.rotations
    }

    /// Signer avec l'identité courante.
    pub fn sign(&self, data: &[u8]) -> [u8; 64] {
        self.current.sign(data)
    }

    /// Vérifier avec la clé courante OU la clé précédente (période de grâce).
    pub fn verify_with_any(&self, pubkey_hex: &str, data: &[u8], sig: &[u8; 64]) -> bool {
        if pubkey_hex == self.current_pubkey_hex() {
            return self.current.verify(data, sig);
        }
        if let Some(prev) = &self.previous {
            if pubkey_hex == prev.pubkey_hex() {
                return prev.verify(data, sig);
            }
        }
        false
    }

    /// Échanger la clé X25519 publique vers un pair (clé courante).
    pub fn x25519_public_key_hex(&self) -> String {
        self.current.x25519_public_key_hex()
    }

    /// Rotation automatique si `now` a dépassé l'intervalle depuis la dernière
    /// rotation. Retourne `true` si une rotation a eu lieu.
    pub fn maybe_rotate(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.last_rotation) < self.rotation_interval_secs {
            return false;
        }
        self.previous = Some(std::mem::replace(&mut self.current, std::mem::replace(
            &mut self.next,
            Identity::generate(),
        )));
        self.last_rotation = now;
        self.rotations += 1;
        true
    }

    /// Force le compteur de rotation (tests / scénario déterministe).
    #[cfg(test)]
    pub fn set_rotation_count(&mut self, count: u64) {
        self.rotations = count;
    }
}
///
/// Format d'un manifeste de build signé :
/// ```text
/// ONDEAPK1 || sha256(apk) || pubkey(dev) || timestamp || signature_ed25519
/// ```
/// où `signature_ed25519` est produite par la clé de l'équipe (root), et le
/// `pubkey(dev)` est la clé qui a signé l'APK au moment du build. La racine de
/// confiance est épinglée (pinning) dans l'application : un APK signé par une
/// clé inconnue est rejeté, ce qui empêche une clé de signature compromise ou
/// un APK non officiel d'être accepté comme mise à jour.
pub const APK_MAGIC: &[u8; 8] = b"ONDEAPK1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApkManifest {
    /// SHA-256 de l'APK
    pub apk_hash: [u8; 32],
    /// Clé publique de développement qui a signé l'APK (32 octets)
    pub dev_pubkey: [u8; 32],
    /// Horodatage du build
    pub timestamp: u64,
}

impl ApkManifest {
    /// Sérialiser le manifeste dans le format signé (avec magic).
    pub fn to_signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 32 + 32 + 8);
        out.extend_from_slice(APK_MAGIC);
        out.extend_from_slice(&self.apk_hash);
        out.extend_from_slice(&self.dev_pubkey);
        out.extend_from_slice(&self.timestamp.to_le_bytes());
        out
    }

    /// Signer le manifeste avec la clé racine de l'équipe.
    pub fn sign(&self, root: &Identity) -> ([u8; 64], Vec<u8>) {
        let bytes = self.to_signed_bytes();
        let sig = root.sign(&bytes);
        (sig, bytes)
    }

    /// Vérifier le manifeste contre la racine de confiance épinglée.
    ///
    /// La signature est vérifiée avec `root_pubkey_bytes` (clé épinglée).
    pub fn verify(root_pubkey_bytes: &[u8; 32], data: &[u8], sig: &[u8; 64]) -> bool {
        if data.len() < 8 || &data[..8] != APK_MAGIC {
            return false;
        }
        // Le manifeste doit mesurer exactement magic + hash + pubkey + ts
        if data.len() != 8 + 32 + 32 + 8 {
            return false;
        }
        Identity::verify_from_pubkey(root_pubkey_bytes, data, sig)
    }

    /// Construire le manifeste d'un APK (hash du fichier).
    pub fn from_apk(apk_bytes: &[u8], dev_pubkey: [u8; 32], timestamp: u64) -> Self {
        let apk_hash = Sha256::digest(apk_bytes);
        Self {
            apk_hash: apk_hash.into(),
            dev_pubkey,
            timestamp,
        }
    }
}

/// Vérification complète d'un APK reçu (Audit #13 — distribution BT non
/// sécurisée). Enchaîne les trois contrôles de la chaîne de confiance :
///
/// 1. La signature Ed25519 du manifeste est valide **et** produite par la
///    racine épinglée (`root_pubkey_bytes`) — une clé inconnue est rejetée ;
/// 2. Le manifeste est bien formé (magic + SHA-256 + pubkey dev + timestamp) ;
/// 3. Le SHA-256 de l'APK reçu correspond au hash du manifeste — un APK
///    falsifié est rejeté même si la signature est valide.
///
/// Retourne le manifeste vérifié (contenant la clé du développeur et le
/// timestamp de build) ou une erreur descriptive.
pub fn verify_apk_signature(
    apk_bytes: &[u8],
    manifest_bytes: &[u8],
    signature: &[u8; 64],
    root_pubkey_bytes: &[u8; 32],
) -> Result<ApkManifest, String> {
    // 1. Signature valide contre la racine de confiance épinglée
    if !ApkManifest::verify(root_pubkey_bytes, manifest_bytes, signature) {
        return Err("APK signature invalid or signed by an untrusted key".to_string());
    }
    // 2. Parse du manifeste (magic || sha256(apk) || dev_pubkey || timestamp)
    if manifest_bytes.len() != 8 + 32 + 32 + 8 || &manifest_bytes[..8] != APK_MAGIC {
        return Err("Malformed APK manifest".to_string());
    }
    let mut apk_hash = [0u8; 32];
    apk_hash.copy_from_slice(&manifest_bytes[8..40]);
    let mut dev_pubkey = [0u8; 32];
    dev_pubkey.copy_from_slice(&manifest_bytes[40..72]);
    let timestamp = u64::from_le_bytes(
        manifest_bytes[72..80]
            .try_into()
            .map_err(|_| "Malformed APK manifest timestamp".to_string())?,
    );
    let manifest = ApkManifest {
        apk_hash,
        dev_pubkey,
        timestamp,
    };
    // 3. Le hash de l'APK reçu correspond au manifeste signé
    let computed: [u8; 32] = Sha256::digest(apk_bytes).into();
    if computed != manifest.apk_hash {
        return Err("APK content hash mismatch (tampered APK)".to_string());
    }
    Ok(manifest)
}

/// Padding de trafic (Audit #14) — uniformise la taille des messages pour
/// masquer la longueur réelle du contenu aux observateurs du mesh.
pub struct TrafficPadding;

impl TrafficPadding {
    /// Tailles de seau : petits messages → 256 B, moyens → 1 Ko,
    /// gros → 4 Ko, très gros → 16 Ko. La longueur observée est toujours
    /// une de ces valeurs, jamais la longueur réelle.
    pub const BUCKETS: [usize; 4] = [256, 1024, 4096, 16_384];

    /// Choisir le seau minimal contenant `len` octets.
    pub fn bucket_for(len: usize) -> usize {
        for b in Self::BUCKETS {
            if len <= b {
                return b;
            }
        }
        Self::BUCKETS[Self::BUCKETS.len() - 1]
    }

    /// Pad un message à la taille du seau (suffixe `0x00`).
    ///
    /// ⚠️ **Pas de troncature (Phase 1.3)** : un message plus gros que le plus
    /// grand seau (`16_384` B) est transmis tel quel, **jamais tronqué** —
    /// tronquer serait une perte de données silencieuse. Le seau maximal
    /// représente la borne au-delà de laquelle le padding n'apporte plus rien
    /// (la taille réelle est déjà révélée). `unpad(pad(x)) == x` pour TOUT `x`.
    pub fn pad(data: &[u8]) -> Vec<u8> {
        let target = Self::bucket_for(data.len());
        let mut out = data.to_vec();
        if target > out.len() {
            out.resize(target, 0x00);
        }
        out
    }

    /// Retirer le padding (tronque les zéros de fin).
    ///
    /// **Idempotent** : `unpad(unpad(x)) == unpad(x)` (tous les zéros de fin
    /// sont retirés en un seul passage). **Tolérant** : un message sans zéro
    /// de fin (non padé) est retourné identique ; un message vide retourne un
    /// slice vide (jamais de panique, y compris pour `len == 0`).
    pub fn unpad(data: &[u8]) -> &[u8] {
        let mut end = data.len();
        while end > 0 && data[end - 1] == 0x00 {
            end -= 1;
        }
        &data[..end]
    }
}

/*
 * ZK Transaction Engine — Mina-style asynchronous offline transactions
 *
 * Uses simplified ZK proofs (in production: mina-rs or zkSync circuits)
 * Transactions are queued locally and pushed to blockchain when internet available
 */

/// ZK Transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkTransaction {
    pub tx_id: String,
    /// Sender's public key
    pub sender: String,
    /// Receiver's public key
    pub receiver: String,
    /// Amount in micro-credits (1e-6)
    pub amount_micro: u64,
    /// Nonce (prevents replay)
    pub nonce: u64,
    /// Timestamp
    pub timestamp: u64,
    /// Simplified ZK proof (in production: full SNARK)
    pub zk_proof: ZkProof,
    /// Whether committed to chain
    pub committed: bool,
}

/// Simplified ZK Proof of Balance
///
/// ⚠️ **STATUT : MOCK — PAS une vraie preuve SNARK.**
///
/// Ce type n'implémente **pas** une preuve à divulgation nulle réelle
/// (Groth16/Plonk/STARK). `prove()` fabrique un engagement SHA-256
/// déterministe et `verify()` ne contrôle que l'**intégrité structurelle**
/// de la preuve (cohérence entre `commitment` et `proof_data`). Un attaquant
/// qui connaît le format peut produire une "preuve" valide sans posséder de
/// solde : **ne jamais utiliser ce module comme contrôle de sécurité pour de
/// la valeur réelle** (ni ici, ni dans un futur bridge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkProof {
    pub commitment: String,
    pub public_inputs: Vec<String>,
    /// In production: full Groth16/Plonk proof
    pub proof_data: Vec<u8>,
}

impl ZkProof {
    /// Create a simplified ZK proof (mock for simulation)
    pub fn prove(sender: &str, receiver: &str, amount_micro: u64, nonce: u64) -> Self {
        let data = format!("{sender}:{receiver}:{amount_micro}:{nonce}");
        let digest: [u8; 32] = Sha256::digest(data.as_bytes()).into();
        let commitment = hex::encode(digest);

        Self {
            commitment: commitment.clone(),
            public_inputs: vec![sender.to_string(), receiver.to_string()],
            // En production : preuve SNARK (Groth16/Plonk) sérialisée.
            // Ici (mock) : le digest SHA-256 brut, engagement du message.
            proof_data: digest.to_vec(),
        }
    }

    /// Verify the proof (MOCK — vérification d'intégrité structurelle).
    ///
    /// Contrôle que la preuve est bien formée : `commitment` est un SHA-256
    /// hex (64 caractères hexadécimaux) et `proof_data` est le digest brut
    /// (32 octets) dont `commitment` est l'encodage hexadécimal. Cela rejette
    /// les blobs manifestement forgés mais ne remplace **pas** une
    /// vérification SNARK réelle.
    pub fn verify(&self) -> bool {
        // En production : vraie vérification de circuit SNARK.
        self.commitment.len() == 64
            && self.commitment.chars().all(|c| c.is_ascii_hexdigit())
            && self.proof_data.len() == 32
            && hex::encode(&self.proof_data) == self.commitment
    }
}

impl ZkTransaction {
    pub fn new(sender: &str, receiver: &str, amount_micro: u64, nonce: u64) -> Self {
        let tx_data = format!("{sender}:{receiver}:{amount_micro}:{nonce}");
        // Full 64-character SHA-256 hex ID (no truncation)
        let tx_id = hex::encode(Sha256::digest(tx_data.as_bytes()));

        Self {
            tx_id,
            sender: sender.to_string(),
            receiver: receiver.to_string(),
            amount_micro,
            nonce,
            // Fallback sûr : horloge antérieure à 1970 → 0 (Audit m6).
            // Un timestamp 0 sera visiblement invalide pour un receveur.
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            zk_proof: ZkProof::prove(sender, receiver, amount_micro, nonce),
            committed: false,
        }
    }

    /// Vérification d'intégrité de la preuve contre les champs de la
    /// transaction (MOCK — preuve simplifiée, pas un vrai SNARK).
    ///
    /// Recalcule l'engagement déterministe à partir des champs
    /// (sender, receiver, amount, nonce) et vérifie que la preuve transporte
    /// exactement cet engagement et ces entrées publiques. Détecte les
    /// transactions dont un champ a été modifié après création — c'est le
    /// garde-fou réel utilisé par `TxPool::submit`, qui remplace l'ancien
    /// contrôle "non vide" (B3).
    pub fn has_valid_proof(&self) -> bool {
        if !self.zk_proof.verify() {
            return false;
        }
        let expected_commitment = hex::encode(Sha256::digest(
            format!(
                "{}:{}:{}:{}",
                self.sender, self.receiver, self.amount_micro, self.nonce
            )
            .as_bytes(),
        ));
        self.zk_proof.commitment == expected_commitment
            && self.zk_proof.public_inputs
                == [self.sender.clone(), self.receiver.clone()]
    }
}

/// Transaction pool for offline processing
pub struct TxPool {
    pending: Vec<ZkTransaction>,
    committed: Vec<ZkTransaction>,
    state_roots: Vec<String>,
}

impl TxPool {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            committed: Vec::new(),
            state_roots: vec!["genesis".to_string()],
        }
    }

    /// Next expected nonce for a sender.
    ///
    /// Derived as the highest nonce already seen (pending or committed) + 1,
    /// so replay of an already-processed nonce is rejected. Returns 0 when the
    /// sender has no transactions yet.
    pub fn next_expected_nonce(&self, sender: &str) -> u64 {
        let mut next = 0u64;
        for tx in self.pending.iter().chain(self.committed.iter()) {
            if tx.sender == sender {
                next = next.max(tx.nonce + 1);
            }
        }
        next
    }

    /// Add transaction to pending pool.
    ///
    /// Rejects: invalid ZK proof, duplicate (sender, nonce) already pending,
    /// and nonces behind the expected sequence (already processed).
    ///
    /// ⚠️ **Garde-fou (Audit B3)** : `has_valid_proof` vérifie l'**intégrité**
    /// de la preuve simplifiée (champs cohérents avec l'engagement SHA-256).
    /// Cela détecte les transactions dont un champ a été modifié après
    /// création ou dont la preuve est vide — mais **ce n'est pas une vraie
    /// preuve de solde SNARK**. Ne pas brancher ce pool sur de la valeur
    /// réelle sans remplacer `ZkProof` par une vraie vérification de circuit.
    pub fn submit(&mut self, tx: ZkTransaction) -> Result<(), String> {
        if !tx.has_valid_proof() {
            return Err("Invalid ZK proof".to_string());
        }

        // Reject duplicate (same sender + nonce already pending)
        if self.pending.iter().any(|t| t.sender == tx.sender && t.nonce == tx.nonce) {
            return Err("Duplicate nonce".to_string());
        }

        // Reject nonces behind the expected sequence (already processed)
        let expected = self.next_expected_nonce(&tx.sender);
        if tx.nonce < expected {
            return Err("Stale nonce".to_string());
        }

        self.pending.push(tx);
        Ok(())
    }

    /// Commit pending transactions (when internet available)
    pub fn commit_pending(&mut self, max_batch: usize) -> Vec<ZkTransaction> {
        let batch_size = max_batch.min(self.pending.len());
        let batch: Vec<ZkTransaction> = self.pending.drain(..batch_size).collect();

        let mut committed = Vec::new();
        for mut tx in batch {
            tx.committed = true;
            self.committed.push(tx.clone());
            committed.push(tx);
        }

        if !committed.is_empty() {
            // Update state root
            let last = self.state_roots.last().unwrap().clone();
            let new_root = hex::encode(Sha256::digest(
                format!("{last}:{}", self.committed.len()).as_bytes()
            ));
            self.state_roots.push(new_root);
        }

        committed
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn committed_count(&self) -> usize {
        self.committed.len()
    }

    pub fn state_root(&self) -> Option<&String> {
        self.state_roots.last()
    }
}

impl Default for TxPool {
    fn default() -> Self {
        Self::new()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_sign_verify() {
        let identity = Identity::generate();
        let data = b"test message";
        let sig = identity.sign(data);

        assert!(identity.verify(data, &sig));
        assert!(!identity.verify(b"wrong", &sig));
    }

    #[test]
    fn test_identity_x25519_keypair() {
        let identity = Identity::generate();
        let pub_bytes = identity.x25519_public_key_bytes();
        assert_eq!(pub_bytes.len(), 32);
        assert_eq!(identity.x25519_public_key_hex().len(), 64);
        assert_eq!(identity.x25519_secret_key_bytes().len(), 32);
        // Secret and public keys must be different
        assert_ne!(identity.x25519_secret_key_bytes(), pub_bytes);
        // Two identities must have distinct X25519 public keys
        let other = Identity::generate();
        assert_ne!(identity.x25519_public_key_hex(), other.x25519_public_key_hex());
    }

    #[test]
    fn test_envelope_encrypt_decrypt_roundtrip() {
        let alice = Identity::generate();
        let bob = Identity::generate();

        let message = b"Message confidentiel pour Bob";
        let envelope = EncryptedEnvelope::encrypt(message, &alice, &bob.x25519_public_key_bytes()).unwrap();

        // Envelope carries the ephemeral public key and the sender's X25519 key
        assert_eq!(envelope.eph_public_key.len(), 32);
        assert_eq!(envelope.sender_pubkey, alice.x25519_public_key_bytes());
        assert_eq!(envelope.nonce.len(), 12);
        assert!(!envelope.ciphertext.is_empty());

        // Bob can decrypt
        let decrypted = EncryptedEnvelope::decrypt(&envelope, &bob).unwrap();
        assert_eq!(decrypted, message);

        // Ciphertext differs from plaintext (real encryption, not a zero key)
        assert_ne!(envelope.ciphertext.as_slice(), message);
    }

    #[test]
    fn test_envelope_wrong_recipient_fails() {
        let alice = Identity::generate();
        let bob = Identity::generate();
        let charlie = Identity::generate();

        let envelope = EncryptedEnvelope::encrypt(b"top secret", &alice, &bob.x25519_public_key_bytes()).unwrap();

        // Bob (intended recipient) decrypts fine
        assert!(EncryptedEnvelope::decrypt(&envelope, &bob).is_ok());
        // Charlie (unrelated identity) cannot decrypt
        assert!(EncryptedEnvelope::decrypt(&envelope, &charlie).is_err());
        // Alice (sender) cannot decrypt either — she lacks Bob's secret key
        assert!(EncryptedEnvelope::decrypt(&envelope, &alice).is_err());
    }

    #[test]
    fn test_envelope_tampered_ciphertext_fails() {
        let alice = Identity::generate();
        let bob = Identity::generate();

        let mut envelope = EncryptedEnvelope::encrypt(b"hello", &alice, &bob.x25519_public_key_bytes()).unwrap();
        // Flip a bit in the ciphertext — authentication must fail
        envelope.ciphertext[0] ^= 0x01;
        assert!(EncryptedEnvelope::decrypt(&envelope, &bob).is_err());
    }

    #[test]
    fn test_sender_pubkey_swap_detected() {
        // A malicious relay replaces the advertised sender X25519 key with its
        // own. Because `sender_pubkey` is bound into the AEAD tag, the
        // recipient's authentication must fail (the tag was computed with the
        // original sender key).
        let alice = Identity::generate();
        let bob = Identity::generate();
        let eve = Identity::generate();

        let mut envelope =
            EncryptedEnvelope::encrypt(b"to bob", &alice, &bob.x25519_public_key_bytes())
                .unwrap();

        // Honest path still works
        assert_eq!(
            EncryptedEnvelope::decrypt(&envelope, &bob).unwrap(),
            b"to bob"
        );

        // Relay swaps the sender key → tag mismatch → rejected
        envelope.sender_pubkey = eve.x25519_public_key_bytes();
        assert!(
            EncryptedEnvelope::decrypt(&envelope, &bob).is_err(),
            "a swapped sender_pubkey must be detected via AEAD"
        );
    }

    #[test]
    fn test_from_bytes_deterministic_x25519() {
        // Restoring the same identity twice must yield the SAME X25519 key
        // (deterministic derivation from the Ed25519 seed), so peers that
        // cached the old public key can still decrypt after a restart.
        let alice = Identity::generate();
        let seed = alice.signing_key_bytes();
        let a1 = Identity::from_bytes(&seed);
        let a2 = Identity::from_bytes(&seed);
        assert_eq!(a1.x25519_public_key_hex(), a2.x25519_public_key_hex());
        assert_eq!(a1.x25519_public_key_hex(), alice.x25519_public_key_hex());
        assert_eq!(a1.pubkey_hex(), alice.pubkey_hex());

        // E2E across a restore must round-trip
        let bob = Identity::generate();
        let envelope = EncryptedEnvelope::encrypt(b"after restart", &a1, &bob.x25519_public_key_bytes())
            .unwrap();
        assert_eq!(
            EncryptedEnvelope::decrypt(&envelope, &bob).unwrap(),
            b"after restart"
        );
    }

    #[test]
    fn test_zk_transaction_creation() {
        let tx = ZkTransaction::new("alice", "bob", 1_000_000, 0);
        assert!(tx.zk_proof.verify());
        assert!(!tx.committed);
        // Full 64-character SHA-256 hex ID (no truncation)
        assert_eq!(tx.tx_id.len(), 64);
    }

    #[test]
    fn test_forged_transaction_rejected() {
        // Audit B3 : le garde-fou de `submit` doit rejeter une transaction
        // dont un champ a été modifié après création (preuve incohérente),
        // et pas seulement une preuve vide.
        let mut pool = TxPool::new();

        // Montant falsifié après création → la preuve ne correspond plus
        let mut forged = ZkTransaction::new("alice", "bob", 1_000_000, 0);
        forged.amount_micro = 999_999_999;
        assert!(
            pool.submit(forged).is_err(),
            "transaction with tampered amount must be rejected"
        );
        assert_eq!(pool.pending_count(), 0);

        // Proof_data vidé → rejeté
        let mut empty_proof = ZkTransaction::new("alice", "bob", 100, 0);
        empty_proof.zk_proof.proof_data.clear();
        assert!(
            pool.submit(empty_proof).is_err(),
            "transaction with empty proof must be rejected"
        );

        // Expéditeur falsifié → rejeté
        let mut forged_sender = ZkTransaction::new("alice", "bob", 100, 0);
        forged_sender.sender = "mallory".to_string();
        assert!(
            pool.submit(forged_sender).is_err(),
            "transaction with tampered sender must be rejected"
        );

        // La transaction authentique, elle, passe
        assert!(pool.submit(ZkTransaction::new("alice", "bob", 100, 0)).is_ok());
    }

    #[test]
    fn test_tx_pool_submit_commit() {
        let mut pool = TxPool::new();
        let tx = ZkTransaction::new("alice", "bob", 1_000_000, 0);

        assert!(pool.submit(tx.clone()).is_ok());
        assert_eq!(pool.pending_count(), 1);

        let committed = pool.commit_pending(10);
        assert_eq!(committed.len(), 1);
        assert!(committed[0].committed);
        assert_eq!(pool.pending_count(), 0);
        assert_eq!(pool.committed_count(), 1);
    }

    #[test]
    fn test_tx_pool_stale_and_duplicate_nonces() {
        let mut pool = TxPool::new();

        // Nonce 0 submitted and committed
        pool.submit(ZkTransaction::new("alice", "bob", 100, 0)).unwrap();
        pool.commit_pending(10);
        assert_eq!(pool.next_expected_nonce("alice"), 1);

        // Replay of the committed nonce → rejected (behind the expected sequence)
        let replay = ZkTransaction::new("alice", "bob", 100, 0);
        assert!(pool.submit(replay).is_err(), "Nonce behind expected sequence must be rejected");

        // Nonce 1 is the next expected → accepted
        pool.submit(ZkTransaction::new("alice", "bob", 100, 1)).unwrap();
        assert_eq!(pool.next_expected_nonce("alice"), 2);

        // Same (sender, nonce) already pending → rejected as duplicate
        let dup = ZkTransaction::new("alice", "bob", 999, 1);
        assert!(pool.submit(dup).is_err(), "Duplicate (sender, nonce) must be rejected");

        // Independent nonce sequence per sender
        assert_eq!(pool.next_expected_nonce("carol"), 0);
        assert!(pool.submit(ZkTransaction::new("carol", "alice", 100, 0)).is_ok());
    }

    #[test]
    fn test_fuzzy_timestamp_within_bounds() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        for _ in 0..200 {
            let ts = fuzzy_timestamp_secs();
            assert!(
                ts.abs_diff(now) <= 30,
                "fuzzy timestamp must be within ±30s (got {ts}, now {now})"
            );
        }
    }

    #[test]
    fn test_rotating_identity_rotation_and_grace() {
        let mut rot = RotatingIdentity::new_with_start(60, 0);
        let start_pub = rot.current_pubkey_hex();
        let data = b"message signed under old key";

        let sig_old = rot.sign(data);
        // Période de grâce : l'ancienne clé est acceptée
        assert!(rot.verify_with_any(&start_pub, data, &sig_old));

        // Pas de rotation avant l'intervalle
        assert!(!rot.maybe_rotate(30));

        // Rotation après 60 s
        assert!(rot.maybe_rotate(120));
        assert_ne!(rot.current_pubkey_hex(), start_pub);
        assert_eq!(rot.rotation_count(), 1);

        // Ancienne signature toujours vérifiable pendant la grâce
        assert!(rot.verify_with_any(&start_pub, data, &sig_old));

        // Nouvelle clé signe et vérifie
        let sig_new = rot.sign(data);
        assert!(rot.verify_with_any(&rot.current_pubkey_hex(), data, &sig_new));

        // Une clé inconnue est rejetée
        let stranger = Identity::generate();
        let sig_stranger = stranger.sign(data);
        assert!(!rot.verify_with_any(&stranger.pubkey_hex(), data, &sig_stranger));
    }

    #[test]
    fn test_rotating_identity_forward_secrecy() {
        // Après rotation, un message chiffré vers l'ANCIENNE clé X25519 ne
        // peut pas être déchiffré avec la NOUVELLE clé (forward secrecy).
        let mut rot = RotatingIdentity::new_with_start(60, 0);
        let old_x25519 = rot.x25519_public_key_hex();

        let sender = Identity::generate();
        let old_pub_bytes: [u8; 32] = hex::decode(&old_x25519)
            .expect("valid hex")
            .try_into()
            .expect("32 bytes");
        let envelope = EncryptedEnvelope::encrypt(b"secret to old identity", &sender, &old_pub_bytes)
            .unwrap();

        // Déchiffrement avec l'ancienne clé : OK
        assert!(EncryptedEnvelope::decrypt(&envelope, rot.current()).is_ok());

        // Rotation → nouvelle clé X25519
        rot.maybe_rotate(120);
        assert_ne!(rot.x25519_public_key_hex(), old_x25519);
        // Le même envelope ne se déchiffre plus avec la nouvelle clé
        // (la clé X25519 a changé — l'ancienne session est morte)
        assert!(EncryptedEnvelope::decrypt(&envelope, rot.current()).is_err());
    }

    #[test]
    fn test_apk_manifest_verify_chain() {
        let root = Identity::generate();
        let dev = Identity::generate();

        // Un APK factice (contenu arbitraire)
        let apk: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let manifest = ApkManifest::from_apk(&apk, dev.verifying_key_bytes(), 1_800_000_000);

        let (sig, bytes) = manifest.sign(&root);

        // Vérification avec la racine épinglée : OK
        assert!(ApkManifest::verify(&root.verifying_key_bytes(), &bytes, &sig));

        // Mauvaise racine → rejeté
        let other_root = Identity::generate();
        assert!(!ApkManifest::verify(&other_root.verifying_key_bytes(), &bytes, &sig));

        // Manifeste falsifié (APK différent) → rejeté
        let evil_apk: Vec<u8> = (0..4096u32).map(|i| (i % 249) as u8).collect();
        let evil_manifest = ApkManifest::from_apk(&evil_apk, dev.verifying_key_bytes(), 1_800_000_000);
        let (evil_sig, _evil_bytes) = evil_manifest.sign(&root);
        assert!(!ApkManifest::verify(&root.verifying_key_bytes(), &bytes, &evil_sig));

        // Le hash de l'APK d'origine est bien celui attendu
        let hash: [u8; 32] = Sha256::digest(&apk).into();
        assert_eq!(manifest.apk_hash, hash);
    }

    #[test]
    fn test_verify_apk_signature_full_chain() {
        let root = Identity::generate();
        let dev = Identity::generate();
        let apk: Vec<u8> = (0..8192u32).map(|i| (i % 253) as u8).collect();

        // Build signé par l'équipe
        let manifest = ApkManifest::from_apk(&apk, dev.verifying_key_bytes(), 1_800_000_000);
        let (sig, manifest_bytes) = manifest.sign(&root);

        // APK authentique + signature racine valide → accepté
        let verified = verify_apk_signature(&apk, &manifest_bytes, &sig, &root.verifying_key_bytes())
            .expect("authentic APK must verify");
        assert_eq!(verified.dev_pubkey, dev.verifying_key_bytes());
        assert_eq!(verified.timestamp, 1_800_000_000);

        // APK falsifié (contenu modifié) → rejeté (hash mismatch)
        let evil_apk: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
        assert!(
            verify_apk_signature(&evil_apk, &manifest_bytes, &sig, &root.verifying_key_bytes())
                .is_err(),
            "tampered APK must be rejected"
        );

        // Signature d'une racine inconnue → rejeté (pinning)
        let other_root = Identity::generate();
        assert!(
            verify_apk_signature(&apk, &manifest_bytes, &sig, &other_root.verifying_key_bytes())
                .is_err(),
            "untrusted root must be rejected"
        );

        // Manifeste malformé → rejeté
        assert!(
            verify_apk_signature(&apk, b"garbage", &sig, &root.verifying_key_bytes()).is_err(),
            "malformed manifest must be rejected"
        );
    }

    #[test]
    fn test_traffic_padding_buckets() {
        assert_eq!(TrafficPadding::bucket_for(0), 256);
        assert_eq!(TrafficPadding::bucket_for(100), 256);
        assert_eq!(TrafficPadding::bucket_for(256), 256);
        assert_eq!(TrafficPadding::bucket_for(300), 1024);
        assert_eq!(TrafficPadding::bucket_for(2000), 4096);
        assert_eq!(TrafficPadding::bucket_for(20_000), 16_384);

        // Round-trip : pad puis unpad redonne le message original
        let msg = b"petit message";
        let padded = TrafficPadding::pad(msg);
        assert_eq!(padded.len(), 256);
        assert_eq!(TrafficPadding::unpad(&padded), msg);

        // Un message plus gros va dans un seau plus grand
        let big = vec![0xABu8; 3000];
        let padded_big = TrafficPadding::pad(&big);
        assert_eq!(padded_big.len(), 4096);
        assert_eq!(TrafficPadding::unpad(&padded_big), big.as_slice());
    }

    #[test]
    fn test_traffic_padding_sizes() {
        // Phase 1.3 : tailles padées aux seaux (jamais la taille réelle).
        assert_eq!(TrafficPadding::pad(&[0x01; 100]).len(), 256);
        assert_eq!(TrafficPadding::pad(&[0x02; 2000]).len(), 4096);
        // 20 000 B → seau maximal 16_384 : le message est transmis TEL QUEL
        // (jamais tronqué — tronquer serait une perte de données silencieuse).
        assert_eq!(TrafficPadding::bucket_for(20_000), 16_384);
        let big = vec![0x03; 20_000];
        let padded = TrafficPadding::pad(&big);
        assert_eq!(padded.len(), 20_000, "oversized messages are never truncated");
        assert_eq!(TrafficPadding::unpad(&padded), big.as_slice());
    }

    #[test]
    fn test_traffic_padding_roundtrip_five_sizes() {
        // Round-trip exact sur 5 tailles représentatives (1 B → > 16 Kio).
        for size in [1usize, 100, 1000, 5000, 30_000] {
            let msg = vec![0xA5u8; size];
            let padded = TrafficPadding::pad(&msg);
            // Le contenu seau est toujours >= à la taille réelle (jamais moins).
            assert!(padded.len() >= size, "pad must never shrink the message");
            assert_eq!(
                TrafficPadding::unpad(&padded),
                msg.as_slice(),
                "round-trip must be exact for {size} B"
            );
        }
    }

    #[test]
    fn test_traffic_padding_unpad_non_padded_identical() {
        // Un message sans zéro de fin (non padé) est retourné identique.
        let msg = b"hello sans padding";
        assert_eq!(TrafficPadding::unpad(msg), msg);

        // Un message dont le dernier octet n'est pas zéro n'est pas touché.
        let ends_non_zero = b"payload\x01";
        assert_eq!(TrafficPadding::unpad(ends_non_zero), ends_non_zero);

        // Un message vide ne panique pas et retourne un slice vide.
        let empty: &[u8] = &[];
        assert_eq!(TrafficPadding::unpad(empty), empty);
    }

    #[test]
    fn test_traffic_padding_unpad_idempotent() {
        // unpad(unpad(pad(x))) == unpad(pad(x)) — l'opération est idempotente.
        for size in [1usize, 100, 1000, 5000] {
            let msg = vec![0x5Cu8; size];
            let padded = TrafficPadding::pad(&msg);
            let once = TrafficPadding::unpad(&padded);
            let twice = TrafficPadding::unpad(once);
            assert_eq!(once, msg.as_slice());
            assert_eq!(twice, once, "unpad must be idempotent");
        }
        // Sur un message déjà entièrement de zéros, l'idempotence tient aussi.
        let zeros = vec![0u8; 256];
        let once = TrafficPadding::unpad(&zeros);
        assert_eq!(once, &[][..] as &[u8]);
        assert_eq!(TrafficPadding::unpad(once), once);
    }

    #[test]
    fn test_traffic_padding_empty_message() {
        // Le seau minimal pour un message vide est 256 B.
        assert_eq!(TrafficPadding::bucket_for(0), 256);
        let padded = TrafficPadding::pad(&[]);
        assert_eq!(padded.len(), 256);
        assert_eq!(padded, vec![0u8; 256]);
        assert_eq!(TrafficPadding::unpad(&padded), &[][..] as &[u8]);
        // `unpad` sur une entrée vide ne panique pas (exigence Phase 1.3).
        assert_eq!(TrafficPadding::unpad(&[]), &[][..] as &[u8]);
    }
}
