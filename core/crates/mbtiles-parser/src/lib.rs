//! MBTiles Parser — offline map tile store reader for ONDE
//!
//! Reads [MBTiles 1.3](https://github.com/mapbox/mbtiles-spec/blob/master/1.3/spec.md)
//! files: a single SQLite database holding raster or vector map tiles indexed
//! by `zoom_level`, `tile_column` and `tile_row`.
//!
//! Scope (core part of ROADMAP Phase 2.4): real SQLite opening, metadata
//! reading (`name`, `format`, `bounds`, `minzoom`, `maxzoom`, …), schema
//! validation and per-tile access by z/x/y with TMS↔XYZ conversion.
//! Rendering / UI display is a later iteration — this crate stays UI-free.
//!
//! Design rules:
//! * typed errors, zero panic on untrusted input (all fallible calls return
//!   [`Result`] and SQL is always parameterized);
//! * no new dependency versus the workspace: `rusqlite` (bundled SQLite) and
//!   `tracing` are already in the tree;
//! * unit tests build synthetic MBTiles databases in temporary directories,
//!   so `cargo test --workspace` never needs an external fixture.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

/// MBTiles specification version targeted by this reader.
pub const SPEC_VERSION: &str = "1.3";

/// Absolute zoom ceiling. Guards the `1 << z` arithmetic against absurd
/// values coming from malformed metadata or callers; real-world datasets stay
/// far below (OpenMapTiles tops out at z14, planetary extracts at z15).
pub const MAX_SUPPORTED_ZOOM: u8 = 30;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed error for every failure mode of this crate. No variant panics:
/// callers always receive a structured reason.
#[derive(Debug)]
pub enum MbtilesError {
    /// Underlying filesystem error (open, metadata read…).
    Io(std::io::Error),
    /// Underlying SQLite error (corrupt page, locked file…).
    Sqlite(rusqlite::Error),
    /// The path given to [`MbtilesReader::open`] does not exist.
    FileNotFound(String),
    /// The file exists but is not a SQLite database (bad magic header).
    NotSqlite(String),
    /// A mandatory table/view is absent (e.g. `metadata`).
    MissingTable(String),
    /// The schema exists but does not satisfy MBTiles requirements.
    InvalidSchema(String),
    /// A metadata row is present but cannot be parsed (bounds, zoom…).
    InvalidMetadata(String),
    /// Requested zoom is outside the dataset range (or the absolute cap).
    ZoomOutOfRange {
        requested: u8,
        min: Option<u8>,
        max: Option<u8>,
    },
    /// Requested x/y is outside `[0, 2^z)` for the requested zoom.
    CoordinatesOutOfBounds { zoom: u8, x: u32, y: u32 },
}

impl fmt::Display for MbtilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Sqlite(e) => write!(f, "SQLite error: {e}"),
            Self::FileNotFound(p) => write!(f, "MBTiles file not found: {p}"),
            Self::NotSqlite(p) => write!(f, "not a SQLite database: {p}"),
            Self::MissingTable(t) => {
                write!(
                    f,
                    "missing mandatory table or view `{t}` (MBTiles {SPEC_VERSION})"
                )
            }
            Self::InvalidSchema(d) => write!(f, "invalid MBTiles schema: {d}"),
            Self::InvalidMetadata(d) => write!(f, "invalid MBTiles metadata: {d}"),
            Self::ZoomOutOfRange {
                requested,
                min,
                max,
            } => match (min, max) {
                (Some(min), Some(max)) => {
                    write!(f, "zoom {requested} out of declared range [{min}, {max}]")
                }
                _ => write!(
                    f,
                    "zoom {requested} outside supported range 0..={MAX_SUPPORTED_ZOOM}"
                ),
            },
            Self::CoordinatesOutOfBounds { zoom, x, y } => match u64::from(*zoom) {
                // Defense in depth: this variant may be built on paths where
                // `zoom` was not yet validated; the shift itself must never
                // panic or wrap (regression M1, checker T17).
                z if z > u64::from(MAX_SUPPORTED_ZOOM) => write!(
                    f,
                    "tile ({zoom}/{x}/{y}) out of bounds: zoom above supported cap {MAX_SUPPORTED_ZOOM}"
                ),
                _ => write!(
                    f,
                    "tile ({zoom}/{x}/{y}) out of bounds: x and y must be < {}",
                    1u64 << *zoom
                ),
            },
        }
    }
}

/// Manual impl: `Connection` is not `Debug`, and callers holding a
/// `Result<_, MbtilesError>` need the reader side to be debuggable too.
impl fmt::Debug for MbtilesReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MbtilesReader")
            .field("path", &self.path)
            .field("metadata", &self.metadata)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl std::error::Error for MbtilesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Sqlite(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for MbtilesError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rusqlite::Error> for MbtilesError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

// ---------------------------------------------------------------------------
// Metadata model
// ---------------------------------------------------------------------------

/// Tile encoding declared by the `format` metadata row.
///
/// MBTiles 1.3 mandates `jpg`, `png` and `pbf`; `webp` is accepted because
/// later drafts of the same file layout standardize it and several producers
/// emit it today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileFormat {
    /// JPEG raster tiles (`jpg`).
    Jpeg,
    /// PNG raster tiles (`png`).
    Png,
    /// Mapbox vector tiles (`pbf`), protobuf-encoded.
    VectorPbf,
    /// WebP raster tiles (`webp`, common extension of the format).
    WebP,
}

impl TileFormat {
    /// Parses the raw `format` metadata value.
    ///
    /// # Errors
    ///
    /// Returns [`MbtilesError::InvalidMetadata`] for any value outside the
    /// recognized set — the spec makes `format` mandatory, guessing would hide
    /// corrupt files behind silent misinterpretation.
    pub fn parse(raw: &str) -> Result<Self, MbtilesError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "pbf" => Ok(Self::VectorPbf),
            "webp" => Ok(Self::WebP),
            other => Err(MbtilesError::InvalidMetadata(format!(
                "unsupported `format` value {other:?} (expected jpg, png, pbf or webp)"
            ))),
        }
    }

    /// Canonical short string as stored in the `metadata` table.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::VectorPbf => "pbf",
            Self::WebP => "webp",
        }
    }
}

impl fmt::Display for TileFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Geographic extent declared by the `bounds` metadata row
/// (`west,south,east,north` in decimal degrees, WGS84).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Bounds {
    fn parse(raw: &str) -> Result<Self, MbtilesError> {
        let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(MbtilesError::InvalidMetadata(format!(
                "`bounds` must have 4 comma-separated values, got {raw:?}"
            )));
        }
        let mut nums = [0.0_f64; 4];
        for (slot, part) in nums.iter_mut().zip(parts.iter()) {
            let v: f64 = part.parse().map_err(|_| {
                MbtilesError::InvalidMetadata(format!("non-numeric `bounds` value {part:?}"))
            })?;
            if !v.is_finite() {
                return Err(MbtilesError::InvalidMetadata(format!(
                    "non-finite `bounds` value {part:?}"
                )));
            }
            *slot = v;
        }
        let [west, south, east, north] = nums;
        if !(-180.0..=180.0).contains(&west)
            || !(-180.0..=180.0).contains(&east)
            || !(-90.0..=90.0).contains(&south)
            || !(-90.0..=90.0).contains(&north)
        {
            return Err(MbtilesError::InvalidMetadata(format!(
                "`bounds` {raw:?} outside WGS84 degree ranges"
            )));
        }
        if west > east || south > north {
            return Err(MbtilesError::InvalidMetadata(format!(
                "`bounds` {raw:?}: corners are crossed (need west<=east, south<=north)"
            )));
        }
        Ok(Self {
            west,
            south,
            east,
            north,
        })
    }
}

impl fmt::Display for Bounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{},{},{},{}",
            self.west, self.south, self.east, self.north
        )
    }
}

/// Default view declared by the `center` metadata row (`lon,lat,zoom`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Center {
    pub longitude: f64,
    pub latitude: f64,
    pub zoom: u8,
}

impl Center {
    fn parse(raw: &str) -> Result<Self, MbtilesError> {
        let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
        if parts.len() != 3 {
            return Err(MbtilesError::InvalidMetadata(format!(
                "`center` must have 3 comma-separated values, got {raw:?}"
            )));
        }
        let longitude: f64 = parts[0].parse().map_err(|_| {
            MbtilesError::InvalidMetadata(format!("non-numeric `center` lon {:?}", parts[0]))
        })?;
        let latitude: f64 = parts[1].parse().map_err(|_| {
            MbtilesError::InvalidMetadata(format!("non-numeric `center` lat {:?}", parts[1]))
        })?;
        let zoom: u8 = parts[2].parse().map_err(|_| {
            MbtilesError::InvalidMetadata(format!("non-integer `center` zoom {:?}", parts[2]))
        })?;
        if !longitude.is_finite()
            || !latitude.is_finite()
            || !(-180.0..=180.0).contains(&longitude)
            || !(-90.0..=90.0).contains(&latitude)
        {
            return Err(MbtilesError::InvalidMetadata(format!(
                "`center` {raw:?} outside WGS84 degree ranges"
            )));
        }
        Ok(Self {
            longitude,
            latitude,
            zoom,
        })
    }
}

/// Parsed content of the `metadata` table (MBTiles 1.3 §metadata).
#[derive(Debug, Clone)]
pub struct MbtilesMetadata {
    /// Mandatory human-readable dataset name.
    pub name: String,
    /// Mandatory tile encoding.
    pub format: TileFormat,
    pub bounds: Option<Bounds>,
    pub center: Option<Center>,
    pub min_zoom: Option<u8>,
    pub max_zoom: Option<u8>,
    pub attribution: Option<String>,
    /// `type` row: typically `baselayer` or `overlay`.
    pub layer_type: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    /// Any other row (`json`, vendor keys…), preserved verbatim so nothing is
    /// silently dropped when re-serializing or inspecting foreign datasets.
    pub extra: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// Schema inspection
// ---------------------------------------------------------------------------

/// How the `tiles` source is physically stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TilesKind {
    /// Plain table (the common flattened layout).
    Table,
    /// View over legacy `map` + `images` tables (explicitly allowed by the
    /// spec); unique-index conformance then lives on the `map` table.
    View,
}

/// Facts recorded while validating the schema. Hard failures abort
/// [`MbtilesReader::open`]; everything here is informational context that a
/// caller (or checker) can inspect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaInfo {
    pub tiles_kind: TilesKind,
    pub has_map_table: bool,
    pub has_images_table: bool,
    /// Unique index covering exactly `(zoom_level, tile_column, tile_row)` on
    /// the `tiles` object (absent by construction when it is a view).
    pub has_unique_tile_index: bool,
}

impl SchemaInfo {
    /// True when the layout matches the full MBTiles 1.3 recommendations:
    /// flattened table with its unique index, or a view backed by the legacy
    /// `map`/`images` pair.
    #[must_use]
    pub fn fully_conformant(&self) -> bool {
        match self.tiles_kind {
            TilesKind::Table => self.has_unique_tile_index,
            // A view derives uniqueness from the underlying tables; the spec's
            // canonical view joins `map` (indexed) to `images`.
            TilesKind::View => self.has_map_table && self.has_images_table,
        }
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Read-only handle to an `.mbtiles` SQLite database.
pub struct MbtilesReader {
    path: String,
    conn: Connection,
    metadata: MbtilesMetadata,
    schema: SchemaInfo,
}

impl MbtilesReader {
    /// Opens `path`, validates the schema and loads metadata.
    ///
    /// # Errors
    ///
    /// * [`MbtilesError::FileNotFound`] — path missing;
    /// * [`MbtilesError::NotSqlite`] — file is not a SQLite database;
    /// * [`MbtilesError::MissingTable`] — no `metadata` table;
    /// * [`MbtilesError::InvalidSchema`] — no usable `tiles` source;
    /// * [`MbtilesError::InvalidMetadata`] — mandatory rows missing/unparsable;
    /// * [`MbtilesError::Sqlite`] / [`MbtilesError::Io`] — propagated.
    ///
    /// Note on cost: when a producer omitted `minzoom`/`maxzoom`, the effective
    /// range is derived from one `SELECT MIN(zoom_level), MAX(zoom_level)
    /// FROM tiles` scan. On huge unindexed datasets this reads the whole zoom
    /// column once at open time — bounded to a single aggregate query by
    /// design; callers needing lazy behavior can rely on producers that ship
    /// the metadata rows (the common case for real-world exports).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, MbtilesError> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let meta = std::fs::metadata(path.as_ref()).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => MbtilesError::FileNotFound(path_str.clone()),
            _ => MbtilesError::Io(e),
        })?;
        if meta.is_dir() {
            return Err(MbtilesError::NotSqlite(format!(
                "{path_str} is a directory"
            )));
        }
        tracing::info!("Opening MBTiles file: {path_str} ({} bytes)", meta.len());

        let conn =
            Connection::open_with_flags(&path_str, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        // `Connection::open` succeeds lazily even on non-SQLite bytes; force a
        // cheap touch of the catalog so corruption surfaces here, typed.
        probe_sqlite_catalog(&conn)?;

        let schema = validate_schema(&conn)?;
        let metadata = load_metadata(&conn)?;
        let mut metadata = metadata;

        // Effective zoom range: fall back to what the data actually holds when
        // the producer omitted minzoom/maxzoom rows (common in hand-made files).
        if metadata.min_zoom.is_none() || metadata.max_zoom.is_none() {
            let (lo, hi): (Option<i64>, Option<i64>) = conn.query_row(
                "SELECT MIN(zoom_level), MAX(zoom_level) FROM tiles",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            metadata.min_zoom = metadata.min_zoom.or_else(|| lo.and_then(u8::try_from_i64));
            metadata.max_zoom = metadata.max_zoom.or_else(|| hi.and_then(u8::try_from_i64));
        }

        Ok(Self {
            path: path_str,
            conn,
            metadata,
            schema,
        })
    }

    /// Filesystem path this reader was opened from.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Parsed `metadata` table.
    #[must_use]
    pub fn metadata(&self) -> &MbtilesMetadata {
        &self.metadata
    }

    /// Recorded schema facts (tables, indexes).
    #[must_use]
    pub fn schema(&self) -> &SchemaInfo {
        &self.schema
    }

    /// Number of rows in `tiles`. Cheap enough for health checks.
    ///
    /// # Errors
    /// Propagates [`MbtilesError::Sqlite`] on query failure.
    pub fn tile_count(&self) -> Result<u64, MbtilesError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tiles", [], |row| row.get(0))?;
        Ok(n.max(0) as u64)
    }
}

/// `Option<i64>` → `Option<u8>` with range guard, kept as a named function so
/// the intent reads plainly at the call site.
trait U8FromI64 {
    fn try_from_i64(v: i64) -> Option<Self>
    where
        Self: Sized;
}

impl U8FromI64 for u8 {
    fn try_from_i64(v: i64) -> Option<Self> {
        u8::try_from(v).ok()
    }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Touches the SQLite catalog to distinguish "not a database" from later,
/// unrelated SQL failures.
fn probe_sqlite_catalog(conn: &Connection) -> Result<(), MbtilesError> {
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(|e| match e {
        rusqlite::Error::SqliteFailure(err, detail) => MbtilesError::NotSqlite(match detail {
            Some(d) => format!("file rejected by SQLite ({d})"),
            None => format!("file rejected by SQLite (code {:?})", err.code),
        }),
        other => MbtilesError::Sqlite(other),
    })
}

/// Kind (`table`/`view`) of an object in the catalog, if present.
fn object_kind(conn: &Connection, name: &str) -> Result<Option<&'static str>, MbtilesError> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = ?1",
            [name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(kind.map(|k| match k.as_str() {
        "view" => "view",
        _ => "table",
    }))
}

/// Column names of a table or view, via the `pragma_table_info` table-valued
/// function (plain SELECT — works identically on tables and views).
fn columns_of(conn: &Connection, name: &str) -> Result<Vec<String>, MbtilesError> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?;
    let cols = stmt
        .query_map([name], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(cols)
}

/// True when `object` carries a UNIQUE, non-partial index over exactly
/// `(zoom_level, tile_column, tile_row)`.
fn has_unique_tile_index(conn: &Connection, object: &str) -> Result<bool, MbtilesError> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_index_list(?1)")?;
    let index_names = stmt
        .query_map([object], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for idx in index_names {
        let unique: i64 = conn.query_row(
            "SELECT \"unique\" FROM pragma_index_list(?1) WHERE name = ?2",
            [object, &idx],
            |row| row.get(0),
        )?;
        let partial: i64 = conn.query_row(
            "SELECT partial FROM pragma_index_list(?1) WHERE name = ?2",
            [object, &idx],
            |row| row.get(0),
        )?;
        if unique == 0 || partial != 0 {
            continue;
        }
        let mut col_stmt = conn.prepare("SELECT name FROM pragma_index_info(?1)")?;
        let mut cols = col_stmt
            .query_map([&idx], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        cols.sort_unstable();
        if cols == ["tile_column", "tile_row", "zoom_level"] {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Validates hard schema requirements and records soft facts.
///
/// Hard requirements (open fails without them):
/// * `metadata` table;
/// * a `tiles` **table or view** exposing the four spec columns — the serving
///   surface every ecosystem tool relies on. Legacy `map`+`images`-only files
///   are one documented `CREATE VIEW tiles AS ...` away from compliance.
fn validate_schema(conn: &Connection) -> Result<SchemaInfo, MbtilesError> {
    if object_kind(conn, "metadata")?.is_none() {
        return Err(MbtilesError::MissingTable("metadata".to_string()));
    }
    let has_map = object_kind(conn, "map")?.is_some();
    let has_images = object_kind(conn, "images")?.is_some();

    let tiles_kind = match object_kind(conn, "tiles")? {
        Some("view") => TilesKind::View,
        Some(_) => TilesKind::Table,
        None => {
            if has_map && has_images {
                return Err(MbtilesError::InvalidSchema(
                    concat!(
                        "no `tiles` table or view; legacy `map`+`images` layout detected — ",
                        "create the standard view: CREATE VIEW tiles AS SELECT ",
                        "map.zoom_level AS zoom_level, map.tile_column AS tile_column, ",
                        "map.tile_row AS tile_row, images.tile_data AS tile_data ",
                        "FROM map JOIN images ON images.tile_id = map.tile_id"
                    )
                    .to_string(),
                ));
            }
            return Err(MbtilesError::MissingTable("tiles".to_string()));
        }
    };

    let required = ["zoom_level", "tile_column", "tile_row", "tile_data"];
    let cols = columns_of(conn, "tiles")?;
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|c| !cols.iter().any(|have| have == c))
        .collect();
    if !missing.is_empty() {
        return Err(MbtilesError::InvalidSchema(format!(
            "`tiles` (a {tiles_kind:?}) lacks columns {missing:?}; required: {required:?}"
        )));
    }

    let has_unique_tile_index = has_unique_tile_index(conn, "tiles")?;

    Ok(SchemaInfo {
        tiles_kind,
        has_map_table: has_map,
        has_images_table: has_images,
        has_unique_tile_index,
    })
}

/// Reads and parses the whole `metadata` table.
fn load_metadata(conn: &Connection) -> Result<MbtilesMetadata, MbtilesError> {
    let mut stmt = conn.prepare("SELECT name, value FROM metadata")?;
    let mut rows = stmt.query([])?;
    let mut raw: BTreeMap<String, String> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let value: Option<String> = row.get(1)?;
        if let Some(value) = value {
            raw.entry(key).or_insert(value);
        }
    }

    let name = raw
        .get("name")
        .cloned()
        .ok_or_else(|| MbtilesError::InvalidMetadata("mandatory `name` row missing".to_string()))?;
    if name.trim().is_empty() {
        return Err(MbtilesError::InvalidMetadata(
            "mandatory `name` row is empty".to_string(),
        ));
    }
    let format_raw = raw.get("format").ok_or_else(|| {
        MbtilesError::InvalidMetadata("mandatory `format` row missing".to_string())
    })?;
    let format = TileFormat::parse(format_raw)?;

    let bounds = match raw.get("bounds") {
        Some(s) => Some(Bounds::parse(s)?),
        None => None,
    };
    let center = match raw.get("center") {
        Some(s) => Some(Center::parse(s)?),
        None => None,
    };
    let parse_zoom = |key: &str| -> Result<Option<u8>, MbtilesError> {
        match raw.get(key) {
            Some(s) => {
                let z: u8 = s.trim().parse().map_err(|_| {
                    MbtilesError::InvalidMetadata(format!(
                        "`{key}` value {s:?} is not an integer 0..=255"
                    ))
                })?;
                Ok(Some(z))
            }
            None => Ok(None),
        }
    };
    let min_zoom = parse_zoom("minzoom")?;
    let max_zoom = parse_zoom("maxzoom")?;
    if let (Some(min), Some(max)) = (min_zoom, max_zoom) {
        if min > max {
            return Err(MbtilesError::InvalidMetadata(format!(
                "minzoom {min} greater than maxzoom {max}"
            )));
        }
    }

    let mut take = |key: &str| raw.remove(key);

    Ok(MbtilesMetadata {
        name,
        format,
        bounds,
        center,
        min_zoom,
        max_zoom,
        attribution: take("attribution"),
        layer_type: take("type"),
        version: take("version"),
        description: take("description"),
        extra: raw,
    })
}

// ---------------------------------------------------------------------------
// Tile access
// ---------------------------------------------------------------------------

/// One map tile: raw encoded bytes plus the format declared by the file's
/// `format` metadata row. Bytes are returned verbatim — decoding (raster
/// blitting, protobuf vector parsing) belongs to a later rendering iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// Raw tile payload exactly as stored in SQLite (`tile_data` BLOB).
    pub data: Vec<u8>,
    /// Declared encoding of `data`.
    pub format: TileFormat,
}

/// Converts an XYZ row (Google/Slippy convention, `y = 0` at **north**) into
/// the TMS row physically stored by MBTiles (`y = 0` at **south**).
///
/// Returns `None` when the grid no longer fits `u32` rows (`zoom >= 33`) or
/// when `y` is outside `[0, 2^zoom)` — never panics, never wraps.
#[must_use]
pub fn xyz_row_to_tms(y: u32, zoom: u8) -> Option<u32> {
    let span = 1_u64.checked_shl(u32::from(zoom))?;
    let y = u64::from(y);
    if y >= span {
        return None;
    }
    u32::try_from(span - 1 - y).ok()
}

/// Inverse of [`xyz_row_to_tms`]: TMS row (south-anchored) to XYZ row
/// (north-anchored).
#[must_use]
pub fn tms_row_to_xyz(y: u32, zoom: u8) -> Option<u32> {
    xyz_row_to_tms(y, zoom)
}

impl MbtilesReader {
    /// Fetches the tile at `column`, `row` in **raw MBTiles/TMS** coordinates,
    /// i.e. exactly the values stored in the `tiles` table.
    ///
    /// Returns `Ok(None)` for a valid coordinate with no tile in the dataset
    /// (sparse datasets are normal).
    ///
    /// # Errors
    ///
    /// * [`MbtilesError::ZoomOutOfRange`] — outside the dataset range or the
    ///   absolute [`MAX_SUPPORTED_ZOOM`] cap;
    /// * [`MbtilesError::CoordinatesOutOfBounds`] — x/y outside `[0, 2^z)`;
    /// * [`MbtilesError::Sqlite`] — query failure.
    pub fn get_tile_tms(
        &self,
        zoom: u8,
        column: u32,
        row: u32,
    ) -> Result<Option<Tile>, MbtilesError> {
        self.ensure_tile_coords(zoom, column, row)?;
        let data: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT tile_data FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
                rusqlite::params![zoom, column, row],
                |row| row.get(0),
            )
            .optional()?;
        Ok(data.map(|data| Tile {
            data,
            format: self.metadata.format,
        }))
    }

    /// Fetches the tile at XYZ coordinates (the convention used by Leaflet,
    /// Google Maps and most renderers). Performs the south/north flip against
    /// the TMS storage layout, then delegates to [`Self::get_tile_tms`].
    ///
    /// # Errors
    /// Same contract as [`Self::get_tile_tms`].
    pub fn get_tile_xyz(
        &self,
        zoom: u8,
        column: u32,
        row_xyz: u32,
    ) -> Result<Option<Tile>, MbtilesError> {
        // Enforce the absolute cap BEFORE building any error: the conversion
        // below returns `None` for absurd zooms, which used to fabricate a
        // `CoordinatesOutOfBounds` whose Display could not render safely.
        if zoom > MAX_SUPPORTED_ZOOM {
            return Err(MbtilesError::ZoomOutOfRange {
                requested: zoom,
                min: None,
                max: None,
            });
        }
        let row_tms =
            xyz_row_to_tms(row_xyz, zoom).ok_or(MbtilesError::CoordinatesOutOfBounds {
                zoom,
                x: column,
                y: row_xyz,
            })?;
        self.get_tile_tms(zoom, column, row_tms)
    }

    /// Validates `z/x/y` against the absolute zoom cap, the dataset-declared
    /// zoom range and the per-zoom grid extent.
    fn ensure_tile_coords(&self, zoom: u8, x: u32, y: u32) -> Result<(), MbtilesError> {
        if zoom > MAX_SUPPORTED_ZOOM {
            // Generic range message: the request is absurd regardless of what
            // this particular dataset declares.
            return Err(MbtilesError::ZoomOutOfRange {
                requested: zoom,
                min: None,
                max: None,
            });
        }
        if let Some(min) = self.metadata.min_zoom {
            if zoom < min {
                return Err(MbtilesError::ZoomOutOfRange {
                    requested: zoom,
                    min: self.metadata.min_zoom,
                    max: self.metadata.max_zoom,
                });
            }
        }
        if let Some(max) = self.metadata.max_zoom {
            if zoom > max {
                return Err(MbtilesError::ZoomOutOfRange {
                    requested: zoom,
                    min: self.metadata.min_zoom,
                    max: self.metadata.max_zoom,
                });
            }
        }
        let span = 1_u64 << zoom; // zoom <= MAX_SUPPORTED_ZOOM: no overflow.
        if u64::from(x) >= span || u64::from(y) >= span {
            return Err(MbtilesError::CoordinatesOutOfBounds { zoom, x, y });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests — synthetic MBTiles databases built in temporary directories, so the
// suite is CI-safe with no external fixture.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct TestDb {
        dir: TempDir,
    }

    impl TestDb {
        fn new(tag: &str) -> Self {
            Self {
                dir: tempfile::Builder::new()
                    .prefix(tag)
                    .tempdir()
                    .expect("tempdir"),
            }
        }

        fn path(&self) -> std::path::PathBuf {
            self.dir.path().join("test.mbtiles")
        }
    }

    /// Creates an empty SQLite file at the test path.
    fn bare_db(db: &TestDb) -> Connection {
        Connection::open(db.path()).expect("create db")
    }

    /// Builds a minimal but fully conformant MBTiles 1.3 database:
    /// metadata + tiles table with unique index + legacy map/images pair.
    fn conformant_db(tag: &str) -> TestDb {
        let db = TestDb::new(tag);
        let conn = bare_db(&db);
        conn.execute_batch(
            r#"
            CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
            CREATE TABLE map (
                zoom_level INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row INTEGER NOT NULL,
                tile_id TEXT NOT NULL
            );
            CREATE TABLE images (tile_id TEXT PRIMARY KEY, tile_data BLOB NOT NULL);
            CREATE UNIQUE INDEX map_index ON map (zoom_level, tile_column, tile_row);
            CREATE TABLE tiles (
                zoom_level INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row INTEGER NOT NULL,
                tile_data BLOB NOT NULL
            );
            CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);
            INSERT INTO metadata VALUES ('name', 'test-mbtiles');
            INSERT INTO metadata VALUES ('format', 'png');
            INSERT INTO metadata VALUES ('bounds', '-180,-85,180,85');
            INSERT INTO metadata VALUES ('minzoom', '0');
            INSERT INTO metadata VALUES ('maxzoom', '2');
            INSERT INTO tiles VALUES (0, 0, 0, x'89504E470D0A1A0A0000');
            INSERT INTO images VALUES ('t0', x'89504E470D0A1A0A0000');
            INSERT INTO map VALUES (0, 0, 0, 't0');
            "#,
        )
        .expect("seed conformant db");
        db
    }

    fn insert_metadata(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT INTO metadata (name, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .expect("insert metadata");
    }

    #[test]
    fn opens_conformant_synthetic_file() {
        let db = conformant_db("mbt-ok");
        let reader = MbtilesReader::open(db.path()).expect("open");
        let md = reader.metadata();
        assert_eq!(md.name, "test-mbtiles");
        assert_eq!(md.format, TileFormat::Png);
        assert_eq!(md.min_zoom, Some(0));
        assert_eq!(md.max_zoom, Some(2));
        let bounds = md.bounds.expect("bounds parsed");
        assert!((bounds.west - -180.0).abs() < f64::EPSILON);
        assert_eq!(md.layer_type, None);
        assert!(
            reader.schema().fully_conformant(),
            "schema recorded: {:?}",
            reader.schema()
        );
        assert_eq!(reader.tile_count().expect("count"), 1);
    }

    #[test]
    fn open_reports_missing_file() {
        let err = MbtilesReader::open("/nonexistent/path/never.mbtiles").unwrap_err();
        assert!(matches!(err, MbtilesError::FileNotFound(_)), "{err}");
    }

    #[test]
    fn open_rejects_directory() {
        let dir = TempDir::new().expect("tmp");
        let err = MbtilesReader::open(dir.path()).unwrap_err();
        assert!(matches!(err, MbtilesError::NotSqlite(_)), "{err}");
    }

    #[test]
    fn open_rejects_non_sqlite_bytes() {
        let db = TestDb::new("mbt-html");
        std::fs::write(db.path(), b"<html>definitely not sqlite</html>").expect("write");
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(matches!(err, MbtilesError::NotSqlite(_)), "{err}");
    }

    #[test]
    fn open_requires_metadata_table() {
        let db = TestDb::new("mbt-nometa");
        bare_db(&db)
            .execute_batch("CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);")
            .expect("ddl");
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::MissingTable(ref t) if t == "metadata"),
            "{err}"
        );
    }

    #[test]
    fn open_requires_tiles_source() {
        let db = TestDb::new("mbt-notiles");
        let conn = bare_db(&db);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             INSERT INTO metadata VALUES ('name','x'),('format','png');",
        )
        .expect("ddl");
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::MissingTable(ref t) if t == "tiles"),
            "{err}"
        );
    }

    #[test]
    fn open_suggests_view_for_legacy_layout_without_tiles() {
        let db = TestDb::new("mbt-legacy");
        let conn = bare_db(&db);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             INSERT INTO metadata VALUES ('name','x'),('format','png');
             CREATE TABLE map (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_id TEXT);
             CREATE TABLE images (tile_id TEXT PRIMARY KEY, tile_data BLOB);",
        )
        .expect("ddl");
        let err = MbtilesReader::open(db.path()).unwrap_err();
        match err {
            MbtilesError::InvalidSchema(msg) => {
                assert!(
                    msg.contains("CREATE VIEW tiles"),
                    "remediation hint absent: {msg}"
                );
            }
            other => panic!("expected InvalidSchema, got {other}"),
        }
    }

    #[test]
    fn open_accepts_tiles_as_view_over_legacy_tables() {
        let db = TestDb::new("mbt-view");
        let conn = bare_db(&db);
        conn.execute_batch(
            r#"
            CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
            INSERT INTO metadata VALUES ('name','viewed'),('format','jpg');
            CREATE TABLE map (
                zoom_level INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row INTEGER NOT NULL,
                tile_id TEXT NOT NULL
            );
            CREATE TABLE images (tile_id TEXT PRIMARY KEY, tile_data BLOB NOT NULL);
            CREATE UNIQUE INDEX map_index ON map (zoom_level, tile_column, tile_row);
            INSERT INTO images VALUES ('i1', x'FFD8FF');
            INSERT INTO map VALUES (5, 3, 7, 'i1');
            CREATE VIEW tiles AS
                SELECT map.zoom_level AS zoom_level, map.tile_column AS tile_column,
                       map.tile_row AS tile_row, images.tile_data AS tile_data
                FROM map JOIN images ON images.tile_id = map.tile_id;
            "#,
        )
        .expect("ddl");
        let reader = MbtilesReader::open(db.path()).expect("open");
        let schema = reader.schema();
        assert_eq!(schema.tiles_kind, TilesKind::View);
        assert!(schema.fully_conformant());
        assert!(
            !schema.has_unique_tile_index,
            "views carry no index themselves"
        );
        assert_eq!(reader.metadata().format, TileFormat::Jpeg);
    }

    #[test]
    fn open_rejects_tiles_with_missing_columns() {
        let db = TestDb::new("mbt-badcols");
        let conn = bare_db(&db);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             INSERT INTO metadata VALUES ('name','x'),('format','png');
             CREATE TABLE tiles (zoom_level INTEGER, tile_data BLOB);",
        )
        .expect("ddl");
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(matches!(err, MbtilesError::InvalidSchema(_)), "{err}");
    }

    #[test]
    fn rejects_missing_or_empty_name_and_format() {
        let base = || {
            let db = TestDb::new("mbt-meta");
            let conn = bare_db(&db);
            conn.execute_batch(
                "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);",
            )
            .expect("ddl");
            (db, conn)
        };
        let (db, conn) = base();
        insert_metadata(&conn, "format", "png");
        drop(conn);
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::InvalidMetadata(ref m) if m.contains("`name`")),
            "{err}"
        );

        let (db, conn) = base();
        insert_metadata(&conn, "name", "x");
        insert_metadata(&conn, "format", "svg");
        drop(conn);
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::InvalidMetadata(ref m) if m.contains("format")),
            "{err}"
        );

        let (db, conn) = base();
        insert_metadata(&conn, "name", "   ");
        insert_metadata(&conn, "format", "png");
        drop(conn);
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::InvalidMetadata(ref m) if m.contains("empty")),
            "{err}"
        );
    }

    #[test]
    fn parses_all_supported_formats() {
        for (raw, want) in [
            ("png", TileFormat::Png),
            ("jpg", TileFormat::Jpeg),
            ("jpeg", TileFormat::Jpeg),
            ("pbf", TileFormat::VectorPbf),
            ("webp", TileFormat::WebP),
            ("PNG ", TileFormat::Png),
        ] {
            assert_eq!(TileFormat::parse(raw).expect(raw), want);
        }
        assert!(TileFormat::parse("gif").is_err());
    }

    #[test]
    fn validates_bounds_parsing() {
        assert!(Bounds::parse("-180,-85.05112878,180,85.05112878").is_ok());
        assert!(Bounds::parse("1,2,3").is_err(), "too few fields");
        assert!(Bounds::parse("a,b,c,d").is_err(), "non numeric");
        assert!(Bounds::parse("10,0,-10,0").is_err(), "crossed longitudes");
        assert!(Bounds::parse("0,50,10,20").is_err(), "crossed latitudes");
        assert!(Bounds::parse("-200,0,10,20").is_err(), "lon out of range");
        assert!(Bounds::parse("0,-100,10,20").is_err(), "lat out of range");
        assert!(Bounds::parse("nan,0,10,20").is_err(), "NaN rejected");
    }

    #[test]
    fn validates_center_parsing() {
        assert!(Center::parse("6.14,46.2,10").is_ok());
        assert!(Center::parse("6.14,46.2").is_err());
        assert!(Center::parse("6.14,46.2,ten").is_err());
        assert!(Center::parse("200,0,1").is_err(), "lon out of range");
    }

    #[test]
    fn validates_zoom_rows() {
        let mk = |min: &str, max: &str| {
            let db = TestDb::new("mbt-zoom");
            let conn = bare_db(&db);
            conn.execute_batch(
                "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
                 CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);",
            )
            .expect("ddl");
            insert_metadata(&conn, "name", "z");
            insert_metadata(&conn, "format", "png");
            insert_metadata(&conn, "minzoom", min);
            insert_metadata(&conn, "maxzoom", max);
            (db, conn)
        };
        let (db, conn) = mk("300", "0");
        drop(conn);
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::InvalidMetadata(ref m) if m.contains("minzoom")),
            "{err}"
        );

        let (db, conn) = mk("5", "2");
        drop(conn);
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(
            matches!(err, MbtilesError::InvalidMetadata(ref m) if m.contains("greater than maxzoom")),
            "{err}"
        );

        let (db, conn) = mk("abc", "2");
        drop(conn);
        let err = MbtilesReader::open(db.path()).unwrap_err();
        assert!(matches!(err, MbtilesError::InvalidMetadata(_)), "{err}");
    }

    #[test]
    fn effective_zoom_range_falls_back_to_tiles_content() {
        let db = TestDb::new("mbt-fallback");
        let conn = bare_db(&db);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE tiles (
                zoom_level INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row INTEGER NOT NULL,
                tile_data BLOB NOT NULL
             );
             INSERT INTO metadata VALUES ('name','fallback'),('format','pbf');
             INSERT INTO tiles VALUES (7, 0, 0, x'00');
             INSERT INTO tiles VALUES (9, 1, 1, x'01');",
        )
        .expect("ddl");
        drop(conn);
        let reader = MbtilesReader::open(db.path()).expect("open");
        assert_eq!(reader.metadata().min_zoom, Some(7));
        assert_eq!(reader.metadata().max_zoom, Some(9));
    }

    #[test]
    fn unknown_and_null_metadata_rows_land_in_extra() {
        let db = TestDb::new("mbt-extra");
        let conn = bare_db(&db);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);
             INSERT INTO metadata VALUES ('name','extras'),('format','png');
             INSERT INTO metadata VALUES ('json','{\"vector_layers\":[]}');
             INSERT INTO metadata VALUES ('vendor_key','vendor_value');
             INSERT INTO metadata VALUES ('null_key', NULL);",
        )
        .expect("ddl");
        drop(conn);
        let reader = MbtilesReader::open(db.path()).expect("open");
        let extra = &reader.metadata().extra;
        assert_eq!(
            extra.get("json").map(String::as_str),
            Some("{\"vector_layers\":[]}")
        );
        assert_eq!(
            extra.get("vendor_key").map(String::as_str),
            Some("vendor_value")
        );
        assert!(
            !extra.contains_key("null_key"),
            "NULL rows must not fabricate empty strings"
        );
    }

    #[test]
    fn detects_missing_unique_index_as_soft_finding() {
        let db = TestDb::new("mbt-noidx");
        let conn = bare_db(&db);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);
             INSERT INTO metadata VALUES ('name','noidx'),('format','png');",
        )
        .expect("ddl");
        drop(conn);
        let reader = MbtilesReader::open(db.path()).expect("open still succeeds");
        let schema = reader.schema();
        assert_eq!(schema.tiles_kind, TilesKind::Table);
        assert!(!schema.has_unique_tile_index);
        assert!(!schema.fully_conformant());
    }

    /// Builds a dataset with tiles at every coordinate of zooms 0 and 1 plus
    /// two sparse zoom-2 tiles; payload byte encodes the TMS row so flips are
    /// observable in assertions.
    fn tiled_db(tag: &str) -> TestDb {
        let db = TestDb::new(tag);
        let conn = bare_db(&db);
        conn.execute_batch(
            r#"
            CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
            CREATE TABLE tiles (
                zoom_level INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row INTEGER NOT NULL,
                tile_data BLOB NOT NULL
            );
            CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);
            INSERT INTO metadata VALUES ('name','tiled'),('format','png');
            INSERT INTO metadata VALUES ('minzoom', '0');
            INSERT INTO metadata VALUES ('maxzoom', '2');
            "#,
        )
        .expect("ddl");
        let tiles: Vec<(u8, u32, u32)> = vec![
            (0, 0, 0),
            (1, 0, 0),
            (1, 0, 1),
            (1, 1, 0),
            (1, 1, 1),
            (2, 1, 2),
            (2, 2, 3),
        ];
        for (z, x, y_tms) in &tiles {
            let payload = [
                0x89_u8,
                b'P',
                b'N',
                b'G',
                *z + 10,
                u8::try_from(*x).unwrap_or(0),
                u8::try_from(*y_tms).unwrap_or(0),
            ];
            conn.execute(
                "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![z, x, y_tms, payload],
            )
            .expect("insert tile");
        }
        db
    }

    #[test]
    fn get_tile_tms_returns_stored_bytes_and_none_on_miss() {
        let db = tiled_db("mbt-get-tms");
        let reader = MbtilesReader::open(db.path()).expect("open");
        let hit = reader
            .get_tile_tms(1, 1, 0)
            .expect("query")
            .expect("tile present");
        assert_eq!(hit.format, TileFormat::Png);
        assert_eq!(hit.data, vec![0x89, b'P', b'N', b'G', 11, 1, 0]);
        let miss = reader.get_tile_tms(2, 3, 3).expect("query");
        assert_eq!(miss, None, "sparse datasets return None for valid coords");
    }

    #[test]
    fn get_tile_xyz_flips_the_row_against_storage() {
        let db = tiled_db("mbt-get-xyz");
        let reader = MbtilesReader::open(db.path()).expect("open");
        // Stored at TMS row 1 == XYZ row (2-1-1)=0 for z=1.
        let north = reader
            .get_tile_xyz(1, 0, 0)
            .expect("query")
            .expect("present");
        assert_eq!(north.data, vec![0x89, b'P', b'N', b'G', 11, 0, 1]);
        // Same physical tile through both APIs:
        let via_tms = reader
            .get_tile_tms(1, 0, 1)
            .expect("query")
            .expect("present");
        assert_eq!(north, via_tms);
        // XYZ row 1 at z=1 maps to TMS row 0 — different stored bytes.
        let south_xyz = reader
            .get_tile_xyz(1, 0, 1)
            .expect("query")
            .expect("present");
        assert_ne!(south_xyz.data, north.data);
    }

    #[test]
    fn tms_xyz_conversions_roundtrip_across_zooms() {
        for zoom in 0_u8..=6 {
            let span = 1_u64 << zoom;
            for y in 0..span {
                let y = y as u32;
                let flipped = xyz_row_to_tms(y, zoom).expect("valid input");
                assert!(
                    u64::from(flipped) < span,
                    "flipped row {flipped} escaped grid at z={zoom}"
                );
                assert_eq!(tms_row_to_xyz(flipped, zoom), Some(y), "roundtrip z={zoom}");
            }
            // Identity at the equator rows only when the grid is symmetric;
            // instead verify the known anchor: z=1 north pole (xyz 0) is south
            // pole (tms 1).
        }
        assert_eq!(xyz_row_to_tms(0, 1), Some(1));
        assert_eq!(xyz_row_to_tms(1, 1), Some(0));
        assert_eq!(tms_row_to_xyz(0, 1), Some(1));
    }

    #[test]
    fn conversions_reject_out_of_grid_inputs() {
        assert_eq!(xyz_row_to_tms(2, 1), None, "y beyond 2^z rejected");
        // z=32 still fits u32 rows exactly (last row == u32::MAX):
        assert_eq!(xyz_row_to_tms(0, 32), Some(u32::MAX));
        assert_eq!(tms_row_to_xyz(u32::MAX, 32), Some(0));
        // z=33 overflows the u32 row space -> refused instead of wrapped:
        assert_eq!(xyz_row_to_tms(0, 33), None);
        assert_eq!(tms_row_to_xyz(0, 33), None);
    }

    #[test]
    fn zoom_outside_declared_range_is_an_error() {
        let db = tiled_db("mbt-zoomrng");
        let reader = MbtilesReader::open(db.path()).expect("open");
        let err = reader.get_tile_tms(3, 0, 0).unwrap_err();
        assert!(
            matches!(
                err,
                MbtilesError::ZoomOutOfRange {
                    requested: 3,
                    min: Some(0),
                    max: Some(2)
                }
            ),
            "{err}"
        );
        // Absolute cap even without declared limits:
        let bare = TestDb::new("mbt-zoomcap");
        let conn = bare_db(&bare);
        conn.execute_batch(
            "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE tiles (zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB);
             INSERT INTO metadata VALUES ('name','nocap'),('format','png');",
        )
        .expect("ddl");
        drop(conn);
        let reader = MbtilesReader::open(bare.path()).expect("open");
        let err = reader
            .get_tile_xyz(MAX_SUPPORTED_ZOOM + 1, 0, 0)
            .unwrap_err();
        assert!(
            matches!(
                err,
                MbtilesError::ZoomOutOfRange {
                    requested: 31,
                    min: None,
                    max: None
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("outside supported range"), "{err}");
    }

    #[test]
    fn coordinates_outside_the_grid_are_errors() {
        let db = tiled_db("mbt-grid");
        let reader = MbtilesReader::open(db.path()).expect("open");
        for (z, x, y) in [(1, 2, 0), (1, 0, 2), (0, 1, 0), (2, 4, 0)] {
            let err = reader.get_tile_xyz(z, x, y).unwrap_err();
            assert!(
                matches!(err, MbtilesError::CoordinatesOutOfBounds { .. }),
                "(z={z},x={x},y={y}): {err}"
            );
        }
    }

    /// Regression M1 (checker T17): `CoordinatesOutOfBounds` used to be built
    /// on paths where zoom was not yet validated, and its Display shifted
    /// `1u64 << zoom` — a debug panic for zoom >= 64. Rendering any variant
    /// must now be panic-free for every representable zoom.
    #[test]
    fn coordinates_error_display_never_panics_on_absurd_zoom() {
        for zoom in [63_u8, 64, 65, 100, 200, u8::MAX] {
            let err = MbtilesError::CoordinatesOutOfBounds { zoom, x: 0, y: 0 };
            let rendered = format!("{err}");
            assert!(rendered.contains(&format!("({zoom}/0/0)")), "{rendered}");
        }
    }

    /// Regression M1 (checker T17), public path: `get_tile_xyz` with an absurd
    /// zoom returns the typed zoom error and its message renders safely.
    #[test]
    fn get_tile_xyz_absurd_zoom_is_typed_and_displayable() {
        let db = tiled_db("mbt-absurd");
        let reader = MbtilesReader::open(db.path()).expect("open");
        for zoom in [31_u8, 64, 200] {
            let err = reader.get_tile_xyz(zoom, 0, 0).unwrap_err();
            assert!(
                matches!(err, MbtilesError::ZoomOutOfRange { requested: z, min: None, max: None } if z == zoom),
                "zoom={zoom}: {err}"
            );
            let rendered = format!("{err}");
            assert!(rendered.contains("outside supported range"), "{rendered}");
        }
        // The same absurd request through the raw TMS API stays consistent.
        let err = reader.get_tile_tms(u8::MAX, 0, 0).unwrap_err();
        assert!(
            matches!(err, MbtilesError::ZoomOutOfRange { requested: 255, .. }),
            "{err}"
        );
    }

    #[test]
    fn tile_count_matches_seeded_rows() {
        let db = tiled_db("mbt-count");
        let reader = MbtilesReader::open(db.path()).expect("open");
        assert_eq!(reader.tile_count().expect("count"), 7);
    }

    #[test]
    fn tiles_are_readable_through_a_view() {
        let db = TestDb::new("mbt-viewget");
        let conn = bare_db(&db);
        conn.execute_batch(
            r#"
            CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);
            INSERT INTO metadata VALUES ('name','viewed'),('format','webp'),('minzoom','5'),('maxzoom','5');
            CREATE TABLE map (
                zoom_level INTEGER NOT NULL,
                tile_column INTEGER NOT NULL,
                tile_row INTEGER NOT NULL,
                tile_id TEXT NOT NULL
            );
            CREATE TABLE images (tile_id TEXT PRIMARY KEY, tile_data BLOB NOT NULL);
            CREATE UNIQUE INDEX map_index ON map (zoom_level, tile_column, tile_row);
            INSERT INTO images VALUES ('i1', x'DEADBEEF');
            INSERT INTO map VALUES (5, 3, 7, 'i1');
            CREATE VIEW tiles AS
                SELECT map.zoom_level AS zoom_level, map.tile_column AS tile_column,
                       map.tile_row AS tile_row, images.tile_data AS tile_data
                FROM map JOIN images ON images.tile_id = map.tile_id;
            "#,
        )
        .expect("ddl");
        drop(conn);
        let reader = MbtilesReader::open(db.path()).expect("open");
        let tile = reader
            .get_tile_tms(5, 3, 7)
            .expect("query")
            .expect("present");
        assert_eq!(tile.data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(tile.format, TileFormat::WebP);
        assert_eq!(
            reader.get_tile_xyz(5, 3, 0).expect("q").map(|t| t.data),
            None,
            "TMS row 7 == XYZ row 24 for z=5; xyz row 0 is a miss"
        );
    }

    #[test]
    fn error_display_is_informative() {
        let e = MbtilesError::ZoomOutOfRange {
            requested: 12,
            min: Some(0),
            max: Some(8),
        };
        assert!(e.to_string().contains("[0, 8]"));
        let e = MbtilesError::CoordinatesOutOfBounds {
            zoom: 2,
            x: 4,
            y: 0,
        };
        assert!(e.to_string().contains("(2/4/0)"));
    }
}
