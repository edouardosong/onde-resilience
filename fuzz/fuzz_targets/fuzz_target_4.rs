#![no_main]
use libfuzzer_sys::fuzz_target;
use dtn_router::DtnMessage;
use zim_parser::extract_text_from_html;

/* FAMILY C — Other parsers (light deps): HTML byte parser + DTN JSON wire parse.
 * zim_parser::extract_text_from_html strips tags from arbitrary HTML bytes (may be
 * non-UTF8). DtnMessage::from_wire_bytes decodes arbitrary JSON bytes into a
 * message. Both return String/Result, never panic on malformed input — safe
 * cargo-fuzz entry points for the "other parsers" family. */
fuzz_target!(|data: &[u8]| {
    // (1) HTML byte parser.
    let _ = extract_text_from_html(data);

    // (2) DTN JSON wire parse of arbitrary bytes.
    let _ = DtnMessage::from_wire_bytes(data);
});
