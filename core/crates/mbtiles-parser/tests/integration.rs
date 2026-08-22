//! Integration test against a REAL `.mbtiles` file, kept OUT of the repo.
//!
//! Set `ONDE_MBTILES_FIXTURE=/path/to/file.mbtiles` to enable. Without the
//! variable (CI default) every test reports a skip note and passes, keeping
//! `cargo test --workspace` green and CI-safe — same policy as the ZIM
//! fixture of T11.
//!
//! The reference fixture is generated with the sqlite3 CLI + stdlib Python
//! (see crates/mbtiles-parser/README.md): it is deliberately NOT produced by
//! rusqlite, so reading it back validates cross-producer compatibility.

use mbtiles_parser::{MbtilesError, MbtilesReader, TileFormat};

const ENV_FIXTURE: &str = "ONDE_MBTILES_FIXTURE";

/// Returns the configured fixture path, or `None` when integration testing is
/// disabled. A set-but-unreadable path fails loudly: the operator explicitly
/// asked for that file.
fn fixture_path() -> Option<std::path::PathBuf> {
    let raw = std::env::var_os(ENV_FIXTURE)?;
    if raw.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(&raw);
    if !path.is_file() {
        panic!("{ENV_FIXTURE} points to {raw:?} which is not a readable file");
    }
    Some(path)
}

fn skip(reason: &str) {
    println!("skip: {reason}");
}

#[test]
fn real_fixture_metadata_schema_and_tiles() {
    let Some(path) = fixture_path() else {
        skip(&format!("set {ENV_FIXTURE} to run this test"));
        return;
    };

    let reader = MbtilesReader::open(&path).expect("fixture must open");

    // --- metadata ---------------------------------------------------------
    let md = reader.metadata();
    assert_eq!(md.name, "onde-test-tiles");
    assert_eq!(md.format, TileFormat::Png);
    assert_eq!(md.min_zoom, Some(0));
    assert_eq!(md.max_zoom, Some(2));
    let bounds = md.bounds.expect("bounds parsed from real file");
    assert!((bounds.west - 5.9).abs() < 1e-9);
    assert!((bounds.south - 45.8).abs() < 1e-9);
    assert!((bounds.east - 10.5).abs() < 1e-9);
    assert!((bounds.north - 47.9).abs() < 1e-9);
    let center = md.center.expect("center parsed");
    assert_eq!(center.zoom, 2);
    assert_eq!(
        md.extra.get("generator").map(String::as_str),
        Some("onde-loop t17 make_fixture.py"),
        "unknown metadata rows are preserved verbatim"
    );

    // --- schema -----------------------------------------------------------
    let schema = reader.schema();
    assert!(schema.has_map_table);
    assert!(schema.has_images_table);
    assert!(schema.has_unique_tile_index, "spec-conformant fixture");
    assert!(schema.fully_conformant());

    // --- tiles ------------------------------------------------------------
    assert_eq!(reader.tile_count().expect("count"), 9);

    // z=0 root tile through both coordinate systems.
    let root_xyz = reader
        .get_tile_xyz(0, 0, 0)
        .expect("query")
        .expect("root tile present at z0");
    assert_eq!(root_xyz.format, TileFormat::Png);
    assert_eq!(&root_xyz.data[..4], &[0x89, b'P', b'N', b'G'], "PNG magic");
    let root_tms = reader
        .get_tile_tms(0, 0, 0)
        .expect("query")
        .expect("root tile present at z0");
    assert_eq!(root_xyz.data, root_tms.data, "z0 has no flip");

    // XYZ row flip observable on a real file: stored TMS row 1 == xyz row 0.
    let north = reader.get_tile_xyz(1, 1, 0).expect("q").expect("present");
    let same_physical = reader.get_tile_tms(1, 1, 1).expect("q").expect("present");
    assert_eq!(north.data, same_physical.data);

    // Distinct tiles carry distinct pixels (generator encodes coords in RGB).
    assert_ne!(
        north.data[16..],
        root_xyz.data[16..],
        "pixel payloads differ"
    );

    // Sparse miss inside a valid grid.
    let miss = reader.get_tile_xyz(2, 3, 3).expect("q");
    assert_eq!(miss, None);

    // Typed guards.
    let zoom_err = reader.get_tile_xyz(3, 0, 0).unwrap_err();
    assert!(
        matches!(zoom_err, MbtilesError::ZoomOutOfRange { requested: 3, .. }),
        "{zoom_err}"
    );
    let coord_err = reader.get_tile_tms(2, 4, 0).unwrap_err();
    assert!(
        matches!(
            coord_err,
            MbtilesError::CoordinatesOutOfBounds {
                zoom: 2,
                x: 4,
                y: 0
            }
        ),
        "{coord_err}"
    );
}

#[test]
fn real_fixture_rejects_lookalikes() {
    // Guards stay active against files that merely exist nearby: opening a
    // text file or an absent path keeps producing typed errors.
    let err = MbtilesReader::open("/nonexistent/definitely-absent.mbtiles").unwrap_err();
    assert!(matches!(err, MbtilesError::FileNotFound(_)), "{err}");
}
