#![no_main]
use libfuzzer_sys::fuzz_target;
use onde_core::protocol::MeshEvent;

/* FAMILY A — Protocol / parsing: the wire parser for unreliable input.
 * MeshEvent::from_wire_bytes is the unique receive point on the DTN flux: every
 * inbound byte stream is decoded here before validation/coding. It takes
 * arbitrary (possibly truncated, all-zero, non-UTF8) bytes and returns a Result,
 * never panic. We also re-run validate / to_wire_bytes / compute_pow on the
 * parsed event to drive downstream parsing paths with the same adversarial input. */
fuzz_target!(|data: &[u8]| {
    let mut ev = MeshEvent::from_wire_bytes(data);

    // (1) Parse entry point, then validation + serialization. Match on `&mut ev`
    //     so compute_pow (&mut self) can borrow the event mutably.
    if let Ok(e) = &mut ev {
        let _ = e.validate();
        let _ = e.to_wire_bytes();
        let _ = e.compute_pow(64);
        let _ = e.is_expired(u64::MAX);
    }

    // (2) Round-trip: serialize then parse again, stressing both directions.
    if let Ok(e) = &ev {
        if let Ok(wire) = e.to_wire_bytes() {
            let _ = MeshEvent::from_wire_bytes(&wire);
        }
    }
});
