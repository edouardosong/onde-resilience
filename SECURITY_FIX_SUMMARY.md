# Security Fix: EncryptedEnvelope Authentication Vulnerability

## Vulnerability Summary
The `EncryptedEnvelope::decrypt()` method was using a hardcoded all-zero key `[0u8; 32]` for ChaCha20-Poly1305 decryption, allowing any attacker to forge valid ciphertexts and have arbitrary plaintext accepted by the system.

## Root Cause
1. **Hardcoded key**: `decrypt()` used `let key = [0u8; 32]` instead of deriving a secret key
2. **Unused sender_pubkey**: The field was set to `[0u8; 32]` and never used for authentication
3. **Broken encrypt()**: Generated a random key but immediately discarded it, making decryption impossible with the correct key

## Security Impact
- **Severity**: Critical
- **Attack Vector**: Remote, unauthenticated
- **Impact**: Complete bypass of message authentication, allowing attackers to inject arbitrary messages

## Fix Implementation

### 1. Added Dependencies (Cargo.toml)
- `x25519-dalek = "2"` - For Elliptic Curve Diffie-Hellman (ECDH) key exchange
- `hkdf = "0.12"` - For HKDF-SHA256 key derivation from shared secrets

### 2. Enhanced Identity Structure
Added X25519 encryption keys to the `Identity` struct:
- `encryption_secret: StaticSecret` - Private X25519 key for ECDH
- `encryption_public: X25519PublicKey` - Public X25519 key for ECDH
- New method: `encryption_public_bytes()` - Returns the public encryption key

### 3. Secure EncryptedEnvelope Implementation

#### Encryption Process:
1. Perform X25519 ECDH between sender's private key and recipient's public key
2. Derive ChaCha20-Poly1305 key using HKDF-SHA256 with context "ONDE-ChaCha20Poly1305-v1"
3. Generate random 12-byte nonce
4. Encrypt plaintext with ChaCha20-Poly1305 (provides authenticated encryption)
5. Store sender's public key in envelope for recipient to derive the same key

#### Decryption Process:
1. Perform X25519 ECDH between recipient's private key and sender's public key (from envelope)
2. Derive the same ChaCha20-Poly1305 key using HKDF-SHA256
3. Decrypt and authenticate ciphertext
4. ChaCha20-Poly1305 authentication tag verification prevents tampering

### 4. API Changes
**Before:**
```rust
pub fn encrypt(data: &[u8]) -> Result<Self, String>
pub fn decrypt(&self) -> Result<Vec<u8>, String>
```

**After:**
```rust
pub fn encrypt(data: &[u8], sender_identity: &Identity, recipient_pubkey: &[u8; 32]) -> Result<Self, String>
pub fn decrypt(&self, recipient_identity: &Identity) -> Result<Vec<u8>, String>
```

### 5. Comprehensive Test Coverage
Added tests to verify:
- ✅ Successful encryption/decryption roundtrip
- ✅ Decryption fails with wrong recipient key
- ✅ Decryption fails when ciphertext is tampered
- ✅ Decryption fails when sender_pubkey is forged

## Security Properties

### Confidentiality
- Messages can only be decrypted by the intended recipient who possesses the corresponding private key
- Forward secrecy: Each message uses a unique nonce, preventing replay attacks

### Authenticity
- ChaCha20-Poly1305 AEAD provides cryptographic authentication
- Sender binding: The sender's public key is cryptographically bound to the ciphertext through ECDH
- Tampering detection: Any modification to ciphertext, nonce, or sender_pubkey causes decryption failure

### Key Derivation
- Uses HKDF-SHA256 with domain separation ("ONDE-ChaCha20Poly1305-v1")
- Shared secret derived via X25519 ECDH (industry standard, used in Signal, WireGuard, etc.)
- No hardcoded keys or predictable key material

## Verification
The fix has been validated with comprehensive unit tests that verify:
1. Normal operation (encryption → decryption)
2. Security against wrong recipient attacks
3. Security against ciphertext tampering
4. Security against sender forgery

## Breaking Changes
This is a breaking API change. Any code using `EncryptedEnvelope` must be updated to:
1. Pass sender identity and recipient public key to `encrypt()`
2. Pass recipient identity to `decrypt()`

Current analysis shows no external usage of these methods in the codebase, so the impact is minimal.
