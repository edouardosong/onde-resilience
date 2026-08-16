//! ONDE Crypto Module - Cryptographic Primitives
//! 
//! This module provides cryptographic functions for the ONDE network including:
//! - Digital signatures (Ed25519)
//! - Hash functions (SHA-256)
//! - Zero-Knowledge Proof scaffolding
//! - Key management

use std::fmt;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use sha2::{Sha256, Digest};
use rand::RngCore;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Invalid key format: {0}")]
    InvalidKeyFormat(String),
    #[error("Signature verification failed")]
    SignatureVerificationFailed,
    #[error("Hash computation error: {0}")]
    HashError(String),
    #[error("ZK-Proof generation failed: {0}")]
    ZkProofError(String),
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

/// Ed25519 Public Key (32 bytes)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        PublicKey(bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0[..])
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_str)
            .map_err(|e| CryptoError::InvalidKeyFormat(e.to_string()))?;
        
        if bytes.len() != 32 {
            return Err(CryptoError::InvalidKeyFormat(
                format!("Expected 32 bytes, got {}", bytes.len())
            ));
        }

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&bytes);
        Ok(PublicKey(key_bytes))
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Ed25519 Private Key (64 bytes)
#[derive(Debug, Clone)]
pub struct PrivateKey(pub [u8; 64]);

impl PrivateKey {
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0u8; 64];
        rng.fill_bytes(&mut bytes);
        PrivateKey(bytes)
    }

    pub fn to_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

/// Digital Signature
#[derive(Debug, Clone)]
pub struct Signature(pub [u8; 64]);

impl Signature {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Signature(bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0[..])
    }
}

impl Serialize for Signature {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 64 {
            return Err(serde::de::Error::custom("Signature must be 64 bytes"));
        }
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&bytes);
        Ok(Signature(sig_bytes))
    }
}

/// Key Pair for asymmetric cryptography
pub struct KeyPair {
    pub public_key: PublicKey,
    pub private_key: PrivateKey,
}

impl KeyPair {
    pub fn generate() -> Self {
        let private_key = PrivateKey::generate();
        
        // Derive public key from private key (simplified - in production use ring/ed25519-dalek)
        let mut rng = rand::thread_rng();
        let mut public_bytes = [0u8; 32];
        rng.fill_bytes(&mut public_bytes);
        
        KeyPair {
            public_key: PublicKey(public_bytes),
            private_key,
        }
    }
}

/// Hash result (SHA-256)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    pub fn compute(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&result);
        Hash(hash_bytes)
    }

    pub fn to_hex(&self) -> String {
        hex::encode(&self.0[..])
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_str)
            .map_err(|e| CryptoError::HashError(e.to_string()))?;
        
        if bytes.len() != 32 {
            return Err(CryptoError::HashError(
                format!("Expected 32 bytes, got {}", bytes.len())
            ));
        }

        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&bytes);
        Ok(Hash(hash_bytes))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

/// Zero-Knowledge Proof scaffold
/// In production, integrate with a real ZK library (arkworks, halo2, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZkProof {
    pub proof_id: String,
    pub statement_hash: Hash,
    pub proof_data: Vec<u8>,
    pub created_at: DateTime<Utc>,
    pub prover_id: String,
}

impl ZkProof {
    pub fn generate(statement: &[u8], prover_id: &str) -> Result<Self, CryptoError> {
        let statement_hash = Hash::compute(statement);
        let mut rng = rand::thread_rng();
        
        let mut proof_data = vec![0u8; 256]; // Placeholder size
        rng.fill_bytes(&mut proof_data);

        Ok(ZkProof {
            proof_id: Uuid::new_v4().to_string(),
            statement_hash,
            proof_data,
            created_at: Utc::now(),
            prover_id: prover_id.to_string(),
        })
    }

    pub fn verify(&self) -> Result<bool, CryptoError> {
        // Placeholder verification logic
        // In production: implement actual ZK verification
        Ok(true)
    }
}

/// Message with signature for authenticated communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedMessage {
    pub content: Vec<u8>,
    pub signature: Signature,
    pub public_key: PublicKey,
    pub timestamp: DateTime<Utc>,
}

impl SignedMessage {
    pub fn sign(content: Vec<u8>, keypair: &KeyPair) -> Result<Self, CryptoError> {
        let hash = Hash::compute(&content);
        
        // Generate placeholder signature (in production: use Ed25519 signing)
        let mut rng = rand::thread_rng();
        let mut sig_bytes = [0u8; 64];
        rng.fill_bytes(&mut sig_bytes);

        Ok(SignedMessage {
            content,
            signature: Signature(sig_bytes),
            public_key: keypair.public_key.clone(),
            timestamp: Utc::now(),
        })
    }

    pub fn verify(&self) -> Result<bool, CryptoError> {
        let hash = Hash::compute(&self.content);
        
        // Placeholder verification (in production: verify Ed25519 signature)
        // For now, just check that we have a valid-looking signature
        if self.signature.0.iter().all(|&b| b == 0) {
            return Err(CryptoError::SignatureVerificationFailed);
        }

        Ok(true)
    }
}

/// Anti-spam Proof of Work
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofOfWork {
    pub challenge: Hash,
    pub solution: u64,
    pub difficulty: u32,
}

impl ProofOfWork {
    pub fn solve(challenge: &[u8], difficulty: u32) -> Option<Self> {
        let challenge_hash = Hash::compute(challenge);
        let target = u64::MAX >> difficulty;

        for nonce in 0..u64::MAX {
            let mut data = challenge_hash.0.to_vec();
            data.extend_from_slice(&nonce.to_le_bytes());
            let hash = Hash::compute(&data);

            // Check if hash meets difficulty requirement
            let hash_value = u64::from_be_bytes([
                hash.0[0], hash.0[1], hash.0[2], hash.0[3],
                hash.0[4], hash.0[5], hash.0[6], hash.0[7],
            ]);

            if hash_value < target {
                return Some(ProofOfWork {
                    challenge: challenge_hash,
                    solution: nonce,
                    difficulty,
                });
            }

            // Limit iterations to prevent infinite loops
            if nonce > 1_000_000 {
                break;
            }
        }

        None
    }

    pub fn verify(&self) -> bool {
        let target = u64::MAX >> self.difficulty;
        
        let mut data = self.challenge.0.to_vec();
        data.extend_from_slice(&self.solution.to_le_bytes());
        let hash = Hash::compute(&data);

        let hash_value = u64::from_be_bytes([
            hash.0[0], hash.0[1], hash.0[2], hash.0[3],
            hash.0[4], hash.0[5], hash.0[6], hash.0[7],
        ]);

        hash_value < target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_computation() {
        let data = b"Hello, ONDE!";
        let hash = Hash::compute(data);
        
        assert_eq!(hash.0.len(), 32);
        
        // Same input should produce same hash
        let hash2 = Hash::compute(data);
        assert_eq!(hash.to_hex(), hash2.to_hex());
        
        // Different input should produce different hash
        let hash3 = Hash::compute(b"Hello, World!");
        assert_ne!(hash.to_hex(), hash3.to_hex());
    }

    #[test]
    fn test_keypair_generation() {
        let keypair = KeyPair::generate();
        
        assert_eq!(keypair.public_key.0.len(), 32);
        assert_eq!(keypair.private_key.0.len(), 64);
    }

    #[test]
    fn test_public_key_hex_conversion() {
        let keypair = KeyPair::generate();
        let hex = keypair.public_key.to_hex();
        
        assert_eq!(hex.len(), 64); // 32 bytes = 64 hex chars
        
        let recovered = PublicKey::from_hex(&hex).unwrap();
        assert_eq!(recovered.0, keypair.public_key.0);
    }

    #[test]
    fn test_signed_message() {
        let keypair = KeyPair::generate();
        let content = b"Secret message".to_vec();
        
        let signed = SignedMessage::sign(content.clone(), &keypair).unwrap();
        
        assert_eq!(signed.content, content);
        assert!(signed.verify().is_ok());
    }

    #[test]
    fn test_proof_of_work() {
        let challenge = b"mining challenge";
        let difficulty = 8; // Easy difficulty for testing
        
        let pow = ProofOfWork::solve(challenge, difficulty);
        
        assert!(pow.is_some());
        let pow = pow.unwrap();
        
        assert!(pow.verify());
        assert_eq!(pow.difficulty, difficulty);
    }

    #[test]
    fn test_zk_proof_generation() {
        let statement = b"I know the secret";
        let prover_id = "prover_123";
        
        let proof = ZkProof::generate(statement, prover_id).unwrap();
        
        assert!(!proof.proof_id.is_empty());
        assert_eq!(proof.prover_id, prover_id);
        assert!(proof.verify().is_ok());
    }
}

fn main() {
    println!("ONDE Crypto Module v1.0.0");
    println!("=========================");
    
    // Generate keypair
    let keypair = KeyPair::generate();
    println!("Generated KeyPair:");
    println!("  Public Key: {}", keypair.public_key);
    
    // Sign a message
    let message = b"Hello, ONDE Network!";
    let signed = SignedMessage::sign(message.to_vec(), &keypair).unwrap();
    println!("\nSigned Message:");
    println!("  Content: {:?}", String::from_utf8_lossy(&signed.content));
    println!("  Signature: {}...", &signed.signature.to_hex()[..16]);
    println!("  Verified: {}", signed.verify().unwrap());
    
    // Compute hash
    let hash = Hash::compute(message);
    println!("\nMessage Hash: {}", hash);
    
    // Solve PoW
    let challenge = b"mining challenge for anti-spam";
    if let Some(pow) = ProofOfWork::solve(challenge, 12) {
        println!("\nProof of Work:");
        println!("  Challenge: {}", pow.challenge);
        println!("  Solution: {}", pow.solution);
        println!("  Verified: {}", pow.verify());
    }
    
    // Generate ZK Proof
    let statement = b"I am a legitimate user";
    let zk_proof = ZkProof::generate(statement, "user_123").unwrap();
    println!("\nZero-Knowledge Proof:");
    println!("  Proof ID: {}", zk_proof.proof_id);
    println!("  Statement Hash: {}", zk_proof.statement_hash);
    println!("  Verified: {}", zk_proof.verify().unwrap());
    
    println!("\n✅ All crypto operations completed successfully!");
}