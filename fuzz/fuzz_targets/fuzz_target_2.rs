#![no_main]
use libfuzzer_sys::fuzz_target;
use onde_core::crypto::{Identity, ApkManifest};
use onde_core::crypto::{verify_apk_signature, APK_MAGIC};

/* FAMILY B — Crypto: Ed25519 signature verification accepting arbitrary bytes.
 * Identity::verify_from_pubkey verifies a sig against a raw public key (arbitrary
 * 32-byte buffer) and against data — the exact entry point used by the node's
 * incoming-alert/endorsement/update paths. ApkManifest::verify and
 * verify_apk_signature are the APK trust-chain verification, also taking raw
 * pubkey/sig/data bytes. All return bool/Result, never panic. The "clip" copies
 * into a sub-slice of the fixed buffer so short inputs pad with zeros and long
 * inputs truncate — no slice-panic either way. */
fuzz_target!(|data: &[u8]| {
    // Clip arbitrary bytes into fixed-size buffers (sub-slice copy, panic-free).
    let mut pk = [0u8; 32];
    let n_pk = data.len().min(32);
    pk[..n_pk].copy_from_slice(&data[..n_pk]);

    let mut sig = [0u8; 64];
    let n_sig = data.len().min(64);
    sig[..n_sig].copy_from_slice(&data[..n_sig]);

    let mut msg = Vec::with_capacity(data.len().min(256));
    msg.extend_from_slice(&data[..data.len().min(256)]);

    // (1) Verify a signature against a raw, arbitrary pubkey.
    let _ = Identity::verify_from_pubkey(&pk, &msg, &sig);

    // (2) Verify with the built-in verifying key + arbitrary data/sig.
    let id = Identity::generate();
    let _ = id.verify(&msg, &sig);

    // (3) APK manifest verify: magic || sha256 || dev_pubkey || ts, raw sig.
    let _ = ApkManifest::verify(&pk, &msg, &sig);

    // (4) Full APK trust chain: apk bytes + manifest bytes + root pubkey + sig.
    let man_bytes = {
        let mut man = Vec::new();
        man.extend_from_slice(&APK_MAGIC[..]);               // magic 8
        man.extend_from_slice(&data[..data.len().min(32)]); // sha256 (<=32)
        man.resize(8 + 32 + 32, 0u8);                       // dev_pubkey pad to 72
        let first = data.first().copied().unwrap_or(0);     // ts byte; guard empty slice
        let ts = (first as u64).to_le_bytes();
    man.extend_from_slice(&ts[..]);                    // ts -> truncated by parser
        man
    };
    let _ = verify_apk_signature(&msg, &man_bytes, &sig, &pk);
});
