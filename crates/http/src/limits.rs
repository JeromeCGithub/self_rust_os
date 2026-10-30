//! Explicit application resource policy; TLS record sizes remain protocol-defined.
/// Largest individual status/header/chunk-extension/trailer line, excluding CRLF.
pub const LINE: usize = 32 * 1024;
/// Total response header bytes, including status lines and CRLF separators.
pub const HEADERS: usize = 256 * 1024;
/// Total trailer bytes, including CRLF separators.
pub const TRAILERS: usize = 256 * 1024;
/// Maximum request target and redirect Location value in bytes.
pub const TARGET: usize = 16 * 1024;
/// Bounded request/absolute-redirect assembly space, including scheme and authority.
pub const REQUEST: usize = 32 * 1024;
/// Default streamed body budget. Callers may select a different Options::max_body.
pub const BODY: usize = 256 * 1024 * 1024;
