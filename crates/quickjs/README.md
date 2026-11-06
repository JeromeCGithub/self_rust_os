# Restricted QuickJS adapter

This is the P0 native feasibility adapter, not the browser's document runtime yet.
The public Rust API owns one thread-confined realm and returns copied results.
See [architecture, native proof and limitations](../../docs/browser-engine-p0.md).

Host checks must run **outside** the repository's bare-metal Cargo configuration:

```bash
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path /path/to/os/crates/quickjs/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /path/to/os/crates/quickjs/Cargo.toml --target x86_64-unknown-linux-gnu --features freestanding
```

`freestanding` selects the native C allocation/math/format adapter even on the
host. Test executables supply `rustos_qjs_epoch_micros` and `rustos_qjs_abort`.
Native user programs select that adapter automatically through `target_os=none`
and must supply those callbacks too. The former seeds non-cryptographic
Math.random; the latter must terminate the current program without unwinding
across C. Neither is a browser binding or a page-visible OS capability.

The builder currently needs an x86-64 Linux host, GCC/GNU headers and `ar`.
Vendored input integrity is checked before every build. Native execution is
covered by `e2e/browser_p0.py`, using the SDK-based `.test-only` `zjsprobe` program.
Do not import kernel internals or expose foreign JSValue layouts to SDK clients.
