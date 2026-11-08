# RustOS fontdue adaptation

Upstream fontdue0.9.3, downloaded from the Cargo registry archive whose SHA-256 is
`2e57e16b3fe8ff4364c0661fdaac543fb38b29ea9bc9c2f45612d90adf931d2b`.
Original MIT/Apache-2.0/Zlib license files and source are retained. Dependency
versions and rasterizer algorithms remain unchanged.

The local `src/font.rs` patch adds a borrowed immutable `LazyFont` for trusted
embedded application fonts. It validates font tables at construction, reads
advances without compiling outlines, and compiles only the requested glyph when
rasterizing. The original eager `Font` remains an independent parity oracle.
Both paths share the same extracted metrics calculation and unchanged geometry
compiler/rasterizer. rustfmt also reformatted this source file.

The owner supplies immutable bytes valid for the face lifetime. RustOS FontSet
requires static embedded bytes; the existing per-painter512-glyph/1MiB cache owns
coverage, with no global mutable font cache. This is not an untrusted-webfont
sandbox, shaping implementation or font-version upgrade. Source updates must
review the small patch and rerun exact eager/lazy Unicode pixel/metric parity,
allocation-budget and native startup/navigation timing tests.
