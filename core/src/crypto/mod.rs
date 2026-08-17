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

/// Cryptographic identity for ONDE nodes.
///
/// Each identity carries two keypairs:
/// - Ed25519 — signatures / verification (identities, event signing),
/// - X25519 — ECDH key exchange for end-to-end message encryption.
pub struct Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    x25519_secret: X25519StaticSecret,
    x25519_public: X25519PublicKey,
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
        let eph_secret = X25519StaticSecret::random_from_rng(&mut RandOsRng);
        let eph_public = X25519PublicKey::from(&eph_secret);
        let recipient_public = X25519PublicKey::from(*recipient_pub_x25519);
        let shared_secret = eph_secret.diffie_hellman(&recipient_public);
        let key_bytes = Self::derive_key(shared_secret.as_bytes())?;
        let key = Key::from_slice(&key_bytes);
        let cipher = ChaCha20Poly1305::new(key);

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

        let mut buf = envelope.ciphertext.to_vec();
        cipher
            .decrypt_in_place(nonce, &envelope.sender_pubkey, &mut buf)
            .map_err(|e| format!("Decryption failed (sender_pubkey tampered?): {e}"))?;
        Ok(buf)
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
        let commitment = hex::encode(Sha256::digest(data.as_bytes()));

        Self {
            commitment: commitment.clone(),
            public_inputs: vec![sender.to_string(), receiver.to_string()],
            proof_data: commitment.as_bytes().to_vec(),
        }
    }

    /// Verify the proof
    pub fn verify(&self) -> bool {
        // In production: full SNARK verification
        !self.commitment.is_empty() && !self.proof_data.is_empty()
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
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            zk_proof: ZkProof::prove(sender, receiver, amount_micro, nonce),
            committed: false,
        }
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
    pub fn submit(&mut self, tx: ZkTransaction) -> Result<(), String> {
        if !tx.zk_proof.verify() {
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
}
