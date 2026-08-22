# mbtiles-parser

MBTiles 1.3 reader (SQLite tile store) for ONDE offline maps.
Core part of ROADMAP Phase 2.4 — rendering/UI is a later iteration, this crate
stays UI-free by design.

## Scope

* Open a real `.mbtiles` file (read-only SQLite via `rusqlite` `bundled` —
  no new dependency versus the workspace).
* Read the `metadata` table: mandatory `name` + `format` (`jpg`, `png`,
  `pbf`, `webp`), optional `bounds`, `center`, `minzoom`, `maxzoom`,
  `attribution`, `type`, `version`, `description`; every other row is kept
  verbatim in `metadata.extra`.
* Validate the schema:
  * hard requirements — `metadata` table; a `tiles` **table or view**
    exposing `zoom_level, tile_column, tile_row, tile_data`;
  * legacy `map`+`images`-only files are refused with the exact remedial SQL;
  * soft facts recorded in [`SchemaInfo`] — presence of `map`/`images`,
    unique index on `(zoom_level, tile_column, tile_row)`, table-vs-view.
* Fetch tiles by z/x/y with typed guards:
  * `get_tile_tms(z, x, y)` — raw storage coordinates;
  * `get_tile_xyz(z, x, y)` — north-anchored convention (Leaflet & co),
    south/north flip applied against storage;
  * zoom outside the declared range or above `MAX_SUPPORTED_ZOOM = 30`,
    and x/y outside `[0, 2^z)`, yield typed errors, never panics.

Bytes are returned verbatim in `Tile { data, format }`; decoding belongs to
the future rendering iteration.

## Usage

```rust
use mbtiles_parser::MbtilesReader;

let reader = MbtilesReader::open("world.mbtiles")?;
println!("{} tiles", reader.tile_count()?);
if let Some(tile) = reader.get_tile_xyz(5, 16, 11)? {
    // tile.data: raw encoded bytes, tile.format: declared encoding
}
```

## Error policy

One enum [`MbtilesError`], ten variants, `Display` + `source()` implemented,
no panic on untrusted input, all SQL parameterized. Unit tests build synthetic
databases in temporary directories so the suite never needs a fixture.

## Integration fixture (out of repo)

`tests/integration.rs` runs only when `ONDE_MBTILES_FIXTURE=/path/file.mbtiles`
is set; unset (CI default), tests print a skip note and pass — CI-safe.

The reference fixture is deliberately NOT produced by rusqlite (cross-producer
check). Regenerate it with the sqlite3 CLI + stdlib Python:

```python
# save as make_fixture.py, then: python3 make_fixture.py
import os, struct, subprocess, zlib

def png_chunk(kind, payload):
    return struct.pack(">I", len(payload)) + kind + payload \
        + struct.pack(">I", zlib.crc32(kind + payload))

def make_png(z, x, y, size=8):
    r, g, b = (37*(z+1)) % 256, (61*(x+1)) % 256, (97*(y+1)) % 256
    ihdr = png_chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    row = b"\x00" + bytes([r, g, b, 255]) * size
    idat = png_chunk(b"IDAT", zlib.compress(row * size))
    return b"\x89PNG\r\n\x1a\n" + ihdr + idat + png_chunk(b"IEND", b"")

def xyz_to_tms(y, z): return (1 << z) - 1 - y

tiles = [(0, 0, 0)] + [(1, x, y) for x in range(2) for y in range(2)] \
      + [(2, x, y) for x, y in [(2, 1), (1, 1), (2, 2), (1, 2)]]
lines = [
    "BEGIN TRANSACTION;",
    "CREATE TABLE metadata (name TEXT PRIMARY KEY, value TEXT);",
    "CREATE TABLE map (zoom_level INTEGER NOT NULL, tile_column INTEGER NOT NULL,"
    " tile_row INTEGER NOT NULL, tile_id TEXT NOT NULL);",
    "CREATE UNIQUE INDEX map_index ON map (zoom_level, tile_column, tile_row);",
    "CREATE TABLE images (tile_id TEXT PRIMARY KEY, tile_data BLOB NOT NULL);",
    "CREATE TABLE tiles (zoom_level INTEGER NOT NULL, tile_column INTEGER NOT NULL,"
    " tile_row INTEGER NOT NULL, tile_data BLOB NOT NULL);",
    "CREATE UNIQUE INDEX tile_index ON tiles (zoom_level, tile_column, tile_row);",
]
for k, v in [("name","onde-test-tiles"),("format","png"),("version","1.0"),
             ("type","baselayer"),("bounds","5.9,45.8,10.5,47.9"),
             ("center","8.2,46.85,2"),("minzoom","0"),("maxzoom","2"),
             ("attribution","ONDE loop synthetic fixture (public domain)"),
             ("description","MBTiles 1.3 fixture for ONDE T17 integration tests"),
             ("generator","onde-loop t17 make_fixture.py")]:
    lines.append(f"INSERT INTO metadata VALUES ('{k}', '{v}');")
for z, x, y in tiles:
    ty, tid, hexpng = xyz_to_tms(y, z), f"t{z}_{x}_{xyz_to_tms(y,z)}", make_png(z,x,y).hex()
    lines += [f"INSERT INTO images VALUES ('{tid}', x'{hexpng}');",
              f"INSERT INTO map VALUES ({z}, {x}, {ty}, '{tid}');",
              f"INSERT INTO tiles VALUES ({z}, {x}, {ty}, x'{hexpng}');"]
lines.append("COMMIT;")
subprocess.run(["sqlite3", "onde-test-tiles.mbtiles"], input="\n".join(lines),
               text=True, check=True)
```

Then: `ONDE_MBTILES_FIXTURE=$PWD/onde-test-tiles.mbtiles cargo test -p mbtiles-parser`.

## Reference

[MBTiles 1.3 specification](https://github.com/mapbox/mbtiles-spec/blob/master/1.3/spec.md)
