# zim-parser

Classic openZIM reader for ONDE offline Wikipedia/Wikisource archives.
Core part of ROADMAP Phase 2.3 — display/UI is a later iteration, this crate
stays UI-free by design.

## Scope

* Parse the fixed header (magic `KIM\x00`, major version ≤ 6) and locate the
  MIME list + cluster/url/title pointer tables (`header.rs`).
* Resolve dirents (url, title, namespace, mime index, cluster/blob pointers)
  with typed errors on malformed input — no panics on user paths (`entries.rs`).
* Rank access by URL and prefix-title search over the loaded index.
* Extract article content: decode each blob from its cluster according to the
  cluster header byte — `'1'` = none, `'4'` = LZ4 block, `'5'` = zstd frame —
  with exact-length verification against the dirent's declared raw size
  (`content.rs`).

Bytes are returned verbatim in `ZimArticle.content`; HTML→text goes through
the stable `extract_text_from_html` helper.

## Layout convention (classic subset)

This crate targets the classic ZIM layout as implemented by ONDE:

```text
@0   magic b"KIM\x00"
@4   major u16 LE, minor u16 LE
@8   UUID (16 bytes)
@24  header_size u32 LE      (stored; end of the table region in fixtures)
@28  article_count u32 LE
@32  media_count u32 LE
@36  creator\0 publisher\0 title\0 description\0 language\0
p5   MIME list region, exactly article_count*4 bytes
     (header.rs derives cluster_ptr_pos = p5 + ac*4 from this)
...  clusterPtr table (ac u32), urlPtr table (ac u32), titlePtr table (ac u32)
...  dirents: url\0 title\0 ns(1B) mime_index(u16 LE) cluster_index(u32 LE)
             blob_offset_in_cluster(u32 LE) blob_size(u32 LE, RAW length)
...  clusters: [code byte][u32 LE compressed size][payload] ...
```

`blob_offset_in_cluster` is **cluster-relative** (the compression byte is
offset 0) and points at the blob's u32 size field — the first blob therefore
has offset 1. Cluster regions are bounded by the next known structure after
their start (another cluster or a dirent), so interleaved layouts stay safe.

### Out of scope: modern / sliced ZIMs

Files with magic `ZIM\x04` (openZIM ≥ 7 "extended" layout, explicit u64
table pointers in the header) and partial `.slice.zim` extracts — e.g.
`~/onde-models/fixtures/wikisource_de_all_maxi.slice.zim`, a 33 MB slice of a
multi-GB original whose absolute offsets point outside the file — are **not**
read by this crate yet (magic check rejects them with `ZimError::InvalidMagic`).
They remain tracked for a later iteration; the classic subset above is what
the Phase 2.3 core delivers.

## Error policy

One enum [`ZimError`], eight variants, `Display` + `std::error::Error`, no
panic on untrusted input: every slice read is bounds-checked and every
decompression verifies its declared length (LZ4 underflow is caught via a
sentinel tail scan — `lz4_flex` reports overflow but not underflow).

Unit tests build synthetic clusters/files in memory or temp dirs, so the suite
never needs a fixture.

## Integration fixture (out of repo)

`tests/integration.rs` runs only when `ONDE_ZIM_FIXTURE=/path/file.zim` is set;
unset (CI default), tests print a skip note and pass — CI-safe. A set-but-missing
path fails loudly.

The reference fixture (`~/onde-models/fixtures/onde-test-articles.zim`) carries
three articles, one per compression code, and is deliberately produced by
CPython + `lz4.block` + `zstandard`, NOT by this crate — reading it back
validates cross-producer compatibility of all three decode paths. Regenerate:

```sh
# deps: apt-get install python3-lz4  (zstandard via pip/apt)
python3 ~/onde-models/fixtures/zim/make_fixture.py ~/onde-models/fixtures/onde-test-articles.zim
ONDE_ZIM_FIXTURE=~/onde-models/fixtures/onde-test-articles.zim cargo test -p zim-parser
```

Note: `~/onde-models/fixtures/gutenberg_zimlite.zim` is an earlier draft fixture
from a previous T11 attempt; it predates the final layout convention (its
pointer tables do not align with `header.rs`) and is superseded by
`onde-test-articles.zim`.
