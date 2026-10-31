# Userspace TLS and HTTPS

Portal accepts `curl https://google.com/`. The separate CURL app accepts the same
URL at its prompt. The existing streaming `userlib::http::{get,get_with}` API
handles HTTP and HTTPS; enable `userlib`'s `tls` feature in an application's
Cargo manifest. No TLS parsing, certificates or application policy run in Ring 0.

## Layer and privilege split

```text
Ring 3: curl / application
        userlib::http + rustos-http (URLs, DNS, GET, redirects, response framing)
        userlib::tls (socket ownership, deadlines, allocator, entropy adapter)
        rustos-tls / Rustls (handshake, certificate verification, records)
        userlib::net + userlib::system (typed syscalls)
--------------------------- int 0x80 ----------------------------------------
Ring 0: PID ownership, complete page/permission checks, copied syscall records
        smoltcp TCP / IPv4 / Ethernet; e1000 driver and DMA
        RDRAND driver; PC CMOS UTC clock driver
```

The reusable [TLS crate](../crates/tls/src/lib.rs) has no socket or hardware access.
Its `Io` trait supplies deadline-bounded raw reads and complete writes. The
[SDK adapter](../user_programs/userlib/src/tls.rs) upgrades a connected socket;
success transfers ownership, failure leaves it with the caller. Drop attempts
`close_notify` within the existing deadline and always releases the socket.
The kernel's current TCP close remains abortive, so delivery of a final TLS
notification is best effort. No threads or background kernel worker were added.

Rustls' unbuffered connection state machine consumes fragmented records, produces
handshake flights, verifies the peer, and encrypts/decrypts application records.
Fatal handshake errors attempt to send Rustls's queued alert before teardown;
failed sessions reject subsequent application reads/writes. The HTTP adapter
upgrades before sending a GET. Certificate or TLS errors never
retry in plaintext. HTTPS-to-HTTP redirects fail before contacting the destination;
HTTP-to-HTTPS is allowed. HTTP framing still runs over authenticated plaintext.
A raw TCP EOF is a TLS truncation error; TLS EOF requires `close_notify`.
Content-Length/chunked HTTP completion can finish without waiting for TLS EOF.

## Dependencies and crypto profile

Direct TLS pins are Rustls `=0.23.46`, rustls-rustcrypto `=0.0.2-alpha`,
webpki-roots `=1.0.11`, getrandom `=0.2.17`, sha2 `=0.10.9`.
Cargo lockfiles pin transitive versions. `nightly-2025-01-30`, the custom targets
and linker layout remain unchanged. Rustls supports a custom crypto provider and
[does not perform network I/O](https://docs.rs/rustls/0.23.46/rustls/).

The RustCrypto provider works with this `no_std` target without C/assembly OS
integration. Its upstream describes it as experimental and explicitly warns
[against production use](https://github.com/RustCrypto/rustls-rustcrypto).
This OS integration is experimental too; passing interoperability tests does not
establish a production security assessment or constant-time audit.

Enabled profile:

- TLS 1.3, X25519 key exchange; ChaCha20-Poly1305-SHA256 preferred, with
  AES-128-GCM-SHA256 and AES-256-GCM-SHA384 also offered.
- Rustls/WebPKI certificate path, trust anchor, hostname/IP SAN, validity dates,
  signature and server-auth usage checks. Actual verification uses
  rustls-webpki `0.103.16`; the provider's older WebPKI dependency supplies algorithm IDs.
- Mozilla roots compiled from webpki-roots. An application can add one explicit
  DER CA file with `Options { tls_ca_file: Some("/ETC/TEST.CA"), ..defaults }`.
  Default curl exposes no custom-CA switch and never disables verification.
- SNI for DNS hosts, ALPN limited to `http/1.1`, no client certificate.

TLS-enabled apps force scalar AES, POLYVAL, ChaCha20, Poly1305 and Curve25519
backends in their Cargo rustflags; SHA-2 uses its `force-soft` feature. The
native thread scheduler does not save SIMD/FPU registers. Audit final linked binaries
when updating crypto dependencies; compiler target flags alone do not disable
upstream `target_feature` dispatch paths.

No TLS 1.2, HTTP/2, session persistence, early data, client-auth configuration,
online revocation fetching or certificate transparency enforcement is provided.
Sites requiring those features, IPv6, unsupported signature algorithms or a
missing trust anchor can fail. CA updates require a dependency update and rebuild.
There is no `--insecure` mode.

## Platform trust and ABI

[Syscalls 80 and 81](syscalls.md#platform-randomness-and-utc) expose random bytes
and UTC seconds. The kernel reads RDRAND with bounded retries and fails if the
CPU lacks it; no timestamp, MAC, TCP state or deterministic seed substitutes for
entropy. QEMU launchers explicitly request `qemu64,rdrand=on` and UTC RTC time.
The virtualization host and its entropy source are part of this trust model.
A failed later provider RNG request also fails closed; fatal provider/allocator
failures may terminate the process rather than return a recoverable request error.

The CMOS driver takes two matching, update-stable calendar samples, validates BCD/
binary and 12/24-hour formats, and converts UTC to Unix seconds. It never changes
RTC IRQ/PIC configuration. TLS rejects clock values before 2020 or beyond year
9999; no invented timestamp bypasses certificate dates. An in-range but wrong host
clock can still invalidate or incorrectly accept dates: there is no authenticated
clock source, rollback protection or hardware entropy health test here.

## Memory, deadlines and use

TLS-enabled apps reserve a private 16 MiB BSS heap using a synchronized free-list
allocator. Completed/failed requests return TLS allocations to it. Infallible
allocation exhaustion prints a fixed diagnostic and exits that process with code
12; process exit releases its sockets. Kernel process frames/stacks are reclaimed
separately after [execution and TLB quiescence](threading.md).

Initial user stacks are 4 MiB (`0x2c00000..0x3000000`) with unmapped guards; native
workers use separately allocated guarded stacks of 16–256 KiB. Kernel
stacks remain 16 KiB. Two encrypted buffers are bounded at 18,437 bytes each;
queued plaintext is capped at 64 KiB. Application writes are limited to 16 KiB,
and an additional DER CA to 16 KiB. HTTP streams bodies and retains its independent
header/body limits. The default connection deadline covers connect, handshake
and response; blocking SDK I/O sleeps one PIT tick on WouldBlock, allowing other
processes to run. There is no multithreaded TLS runtime or connection pooling.

```rust,no_run
use userlib::{http, sys};
// Cargo.toml: userlib = { path = "../userlib", features = ["tls"] }
let response = http::get("https://google.com/", |bytes| {
    if sys::console_write(bytes) == bytes.len() { Ok(()) }
    else { Err(http::Error::Output) }
})?;
```

Applications needing a raw encrypted stream can use
`userlib::tls::Connection::{upgrade,read,write}` over an owned connected TCP handle.
The upgrade timeout becomes the stream's lifetime deadline; reads/writes do not
reset it. An upgrade failure requires the caller to close its original handle.

The [expanded HTTP and memory limits](http-limits-validation.md) supersede the
initial TLS milestone's 1 MiB heap and 128 KiB stack. QEMU now starts with 1 GiB
RAM; the kernel heap is 64 MiB. These capacities are fixed and eagerly mapped;
there is no demand-paged growing heap or stack.

## Verification

```bash
./tools/build-grub-disk.sh
python3 -B e2e/tls.py --site https://google.com/
python3 -B e2e/tls.py --no-rng
python3 -B e2e/tls.py --bad-clock
# Host crate tests must run outside the root bare-metal Cargo configuration:
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path "$REPO/crates/tls/Cargo.toml" --target x86_64-unknown-linux-gnu
rustc +nightly-2025-01-30 --edition=2021 --test "$REPO/src/drivers/char/random.rs" -o /tmp/rustos-rng-tests
/tmp/rustos-rng-tests
rustc +nightly-2025-01-30 --edition=2021 --test "$REPO/src/drivers/rtc/cmos.rs" -o /tmp/rustos-rtc-tests
/tmp/rustos-rtc-tests
```

The QEMU harness uses independent Python/OpenSSL TLS servers and disposable
certificates. OpenSSL is a host-only test prerequisite. Run HTTP, network, kernel,
memory and desktop checks too; recorded evidence and limitations are in
[TLS validation](tls-validation.md).
