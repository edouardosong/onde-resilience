#![no_main]
use libfuzzer_sys::fuzz_target;
use onde_core::protocol::{MeshEvent, OndeMessageType};
use onde_core::social::SocialPost;

// FAMILY C — Social protocol: fuzz the SocialPost deserialization,
// validation, and wire roundtrip via MeshEvent.
//
// Every social event on the mesh is serialized as JSON inside a
// MeshEvent and deserialized on receipt. This target fuzzes:
//   1. Arbitrary JSON → SocialPost deserialization (no panic).
//   2. Validation of the deserialized SocialPost (no panic).
//   3. MeshEvent roundtrip with SocialPost content.
fuzz_target!(|data: &[u8]| {
    // (1) Direct JSON → SocialPost (best-effort, never panic)
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(post) = serde_json::from_str::<SocialPost>(s) {
            let _ = post.validate();
        }
    }

    // (2) MeshEvent with SocialPost content → decode + validate
    if let Ok(ev) = MeshEvent::from_wire_bytes(data) {
        if ev.kind == OndeMessageType::SocialPost {
            if let Ok(post) = serde_json::from_str::<SocialPost>(&ev.content) {
                let _ = post.validate();
            }
        }
    }
});