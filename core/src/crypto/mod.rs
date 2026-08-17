/// Cryptography — Ed25519 identities, ChaCha20-Poly1305 encryption, ZK transactions

use ed25519_dalek::{SigningKey, VerifyingKey, Verifier, Signer};
use ed25519_dalek::Signature as EdSignature;
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce, KeyInit,
    aead::Aead,
};
use x25519_dalek::{StaticSecret, PublicKey as X25519PublicKey};
use hkdf::Hkdf;
use rand::rngs::OsRng as RandOsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Cryptographic identity for ONDE nodes
pub struct Identity {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
    encryption_secret: StaticSecret,
    encryption_public: X25519PublicKey,
}

impl Identity {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut RandOsRng);
        let verifying_key = signing_key.verifying_key();
        let encryption_secret = StaticSecret::random_from_rng(RandOsRng);
        let encryption_public = X25519PublicKey::from(&encryption_secret);
        Self {
            signing_key,
            verifying_key,
            encryption_secret,
            encryption_public,
        }
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let signing_key = SigningKey::from_bytes(bytes);
        let verifying_key = signing_key.verifying_key();
        // Derive encryption key from signing key for deterministic generation
        let mut encryption_bytes = [0u8; 32];
        let mut hasher = Sha256::new();
        hasher.update(b"ONDE-encryption-key-v1");
        hasher.update(bytes);
        encryption_bytes.copy_from_slice(&hasher.finalize());
        let encryption_secret = StaticSecret::from(encryption_bytes);
        let encryption_public = X25519PublicKey::from(&encryption_secret);
        Self {
            signing_key,
            verifying_key,
            encryption_secret,
            encryption_public,
        }
    }

    pub fn signing_key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    pub fn verifying_key_bytes(&self) -> [u8; 32] {
        self.verifying_key.to_bytes()
    }

    pub fn encryption_public_bytes(&self) -> [u8; 32] {
        self.encryption_public.to_bytes()
    }

    /// Get public key as hex
    pub fn pubkey_hex(&self) -> String {
        hex::encode(self.verifying_key_bytes())
    }

    /// Sign data
    pub fn sign(&self, data: &[u8]) -> [u8; 64] {
        self.signing_key.sign(data).to_bytes()
    }

    /// Verify a signature
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

/// Encrypted message envelope
pub struct EncryptedEnvelope {
    /// ChaCha20-Poly1305 ciphertext
    pub ciphertext: Vec<u8>,
    /// 12-byte nonce
    pub nonce: [u8; 12],
    /// Sender X25519 public key (for ECDH key derivation)
    pub sender_pubkey: [u8; 32],
}

impl EncryptedEnvelope {
    /// Encrypt data for a recipient using ECDH-derived symmetric key
    /// 
    /// # Arguments
    /// * `data` - The plaintext data to encrypt
    /// * `sender_identity` - The sender's Identity (for ECDH)
    /// * `recipient_pubkey` - The recipient's X25519 public key
    /// 
    /// # Security
    /// Uses X25519 ECDH to derive a shared secret, then HKDF-SHA256 to derive
    /// a ChaCha20-Poly1305 key. Each message uses a unique random nonce.
    pub fn encrypt(data: &[u8], sender_identity: &Identity, recipient_pubkey: &[u8; 32]) -> Result<Self, String> {
        // Perform ECDH key exchange
        let recipient_x25519 = X25519PublicKey::from(*recipient_pubkey);
        let shared_secret = sender_identity.encryption_secret.diffie_hellman(&recipient_x25519);
        
        // Derive encryption key using HKDF-SHA256
        let hkdf = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut key_bytes = [0u8; 32];
        hkdf.expand(b"ONDE-ChaCha20Poly1305-v1", &mut key_bytes)
            .map_err(|e| format!("HKDF expansion failed: {}", e))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; 12];
        RandOsRng.fill_bytes(&mut nonce_bytes);

        // Encrypt with ChaCha20-Poly1305
        let key = Key::from_slice(&key_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let cipher = ChaCha20Poly1305::new(key);

        let ciphertext = cipher.encrypt(nonce, data)
            .map_err(|e| format!("Encryption failed: {}", e))?;

        Ok(Self {
            ciphertext,
            nonce: nonce_bytes,
            sender_pubkey: sender_identity.encryption_public_bytes(),
        })
    }

    /// Decrypt the envelope using the recipient's identity
    /// 
    /// # Arguments
    /// * `recipient_identity` - The recipient's Identity (for ECDH)
    /// 
    /// # Security
    /// Derives the same shared secret using ECDH with the sender's public key
    /// from the envelope, then uses HKDF-SHA256 to derive the decryption key.
    /// ChaCha20-Poly1305 provides authenticated encryption, so tampering is detected.
    pub fn decrypt(&self, recipient_identity: &Identity) -> Result<Vec<u8>, String> {
        // Perform ECDH key exchange with sender's public key
        let sender_x25519 = X25519PublicKey::from(self.sender_pubkey);
        let shared_secret = recipient_identity.encryption_secret.diffie_hellman(&sender_x25519);
        
        // Derive decryption key using HKDF-SHA256 (same as encryption)
        let hkdf = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut key_bytes = [0u8; 32];
        hkdf.expand(b"ONDE-ChaCha20Poly1305-v1", &mut key_bytes)
            .map_err(|e| format!("HKDF expansion failed: {}", e))?;

        // Decrypt with ChaCha20-Poly1305
        let key = Key::from_slice(&key_bytes);
        let nonce = Nonce::from_slice(&self.nonce);
        let cipher = ChaCha20Poly1305::new(key);

        cipher.decrypt(nonce, self.ciphertext.as_ref())
            .map_err(|e| format!("Decryption failed (authentication error or wrong key): {}", e))
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
        let tx_id = hex::encode(Sha256::digest(tx_data.as_bytes()));

        Self {
            tx_id: tx_id[..16].to_string(),
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
    current_nonce: std::collections::HashMap<String, u64>,
}

impl TxPool {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            committed: Vec::new(),
            state_roots: vec!["genesis".to_string()],
            current_nonce: std::collections::HashMap::new(),
        }
    }

    /// Add transaction to pending pool
    pub fn submit(&mut self, tx: ZkTransaction) -> Result<(), String> {
        if !tx.zk_proof.verify() {
            return Err("Invalid ZK proof".to_string());
        }

        // Check nonce
        let expected = self.current_nonce.entry(tx.sender.clone()).or_insert(0);
        if tx.nonce < *expected {
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
            *self.current_nonce.entry(tx.sender.clone()).or_insert(0) = tx.nonce + 1;
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
    fn test_encrypted_envelope_roundtrip() {
        // Create sender and recipient identities
        let sender = Identity::generate();
        let recipient = Identity::generate();
        
        let plaintext = b"Secret message for testing";
        
        // Encrypt message from sender to recipient
        let envelope = EncryptedEnvelope::encrypt(
            plaintext,
            &sender,
            &recipient.encryption_public_bytes()
        ).expect("Encryption should succeed");
        
        // Verify sender_pubkey is set correctly
        assert_eq!(envelope.sender_pubkey, sender.encryption_public_bytes());
        
        // Decrypt message as recipient
        let decrypted = envelope.decrypt(&recipient)
            .expect("Decryption should succeed");
        
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypted_envelope_wrong_recipient() {
        // Create sender and two recipients
        let sender = Identity::generate();
        let recipient1 = Identity::generate();
        let recipient2 = Identity::generate();
        
        let plaintext = b"Secret message";
        
        // Encrypt for recipient1
        let envelope = EncryptedEnvelope::encrypt(
            plaintext,
            &sender,
            &recipient1.encryption_public_bytes()
        ).expect("Encryption should succeed");
        
        // Try to decrypt as recipient2 (should fail)
        let result = envelope.decrypt(&recipient2);
        assert!(result.is_err(), "Decryption with wrong key should fail");
    }

    #[test]
    fn test_encrypted_envelope_tampered_ciphertext() {
        let sender = Identity::generate();
        let recipient = Identity::generate();
        
        let plaintext = b"Secret message";
        
        let mut envelope = EncryptedEnvelope::encrypt(
            plaintext,
            &sender,
            &recipient.encryption_public_bytes()
        ).expect("Encryption should succeed");
        
        // Tamper with ciphertext
        if !envelope.ciphertext.is_empty() {
            envelope.ciphertext[0] ^= 0xFF;
        }
        
        // Decryption should fail due to authentication tag mismatch
        let result = envelope.decrypt(&recipient);
        assert!(result.is_err(), "Decryption of tampered ciphertext should fail");
    }

    #[test]
    fn test_encrypted_envelope_forged_sender() {
        let sender = Identity::generate();
        let attacker = Identity::generate();
        let recipient = Identity::generate();
        
        let plaintext = b"Secret message";
        
        let mut envelope = EncryptedEnvelope::encrypt(
            plaintext,
            &sender,
            &recipient.encryption_public_bytes()
        ).expect("Encryption should succeed");
        
        // Attacker tries to forge sender_pubkey
        envelope.sender_pubkey = attacker.encryption_public_bytes();
        
        // Decryption should fail because the key derivation will be wrong
        let result = envelope.decrypt(&recipient);
        assert!(result.is_err(), "Decryption with forged sender should fail");
    }

    #[test]
    fn test_zk_transaction_creation() {
        let tx = ZkTransaction::new("alice", "bob", 1_000_000, 0);
        assert!(tx.zk_proof.verify());
        assert!(!tx.committed);
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
}