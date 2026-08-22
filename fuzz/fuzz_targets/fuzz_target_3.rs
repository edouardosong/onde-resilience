#![no_main]
use libfuzzer_sys::fuzz_target;
use onde_core::crypto::{Identity, EncryptedEnvelope};

/* FAMILY B — Crypto: ChaCha20-Poly1305 AEAD envelope with arbitrary bytes.
 * decrypt takes an arbitrary envelope (ephemeral pubkey, nonce, sender pubkey,
 * ciphertext) and re-derives the key via X25519 ECDH + HKDF, then runs
 * decrypt_in_place. A tampered nonce/ciphertext/sender_pubkey must yield an AEAD
 * auth failure (Result::Err), never a panic. encrypt is the symmetric counterpart
 * accepting message bytes + recipient pubkey. Both return Result. */
fuzz_target!(|data: &[u8]| {
    let id = Identity::generate();

    // Encrypt arbitrary bytes for an arbitrary recipient key.
    let mut recpk = [0u8; 32];
    recpk[..data.len().min(32)].copy_from_slice(&data[..data.len().min(32)]);
    if let Ok(env) = EncryptedEnvelope::encrypt(&data, &id, &recpk) {
        // Decrypt the freshly-created envelope (round-trip sanity).
        let _ = EncryptedEnvelope::decrypt(&env, &id);
    }

    // Build an arbitrary envelope and decrypt it as the recipient.
    let mut eph = [0u8; 32];
    let n1 = data.len().min(32);
    eph[..n1].copy_from_slice(&data[..n1]);

    let mut nonce = [0u8; 12];
    let n2 = data.len().min(12);
    nonce[..n2].copy_from_slice(&data[..n2]);

    let mut sender = [0u8; 32];
    let n3 = data.len().min(32);
    sender[..n3].copy_from_slice(&data[..n3]);

    let mut ct = Vec::new();
    ct.extend_from_slice(&data[..data.len().min(512)]);

    let env = EncryptedEnvelope {
        ciphertext: ct,
        nonce,
        sender_pubkey: sender,
        eph_public_key: eph,
    };
    // Recipient re-derives key from arbitrary eph pubkey + AEAD decrypt.
    let _ = EncryptedEnvelope::decrypt(&env, &id);
});
