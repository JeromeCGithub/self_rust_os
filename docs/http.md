# Userspace HTTP client and curl

HTTP and DNS execute entirely in Ring 3. The kernel's existing ICMP/UDP/TCP ABI
preserves syscalls 20–26. The reusable no_std [protocol crate](../crates/http/src/lib.rs)
handles URLs, DNS A/CNAME responses, TCP request writes, HTTP response framing,
redirects and deadlines. The [SDK adapter](../user_programs/userlib/src/http.rs)
uses typed socket syscalls and sleeps between retries. It owns socket cleanup on
success, failure and redirect. No kernel or driver imports are used by HTTP.

## Commands

In Portal:

```text
curl https://google.com/
curl http://10.0.2.2:8000/path
```

Portal prints the decoded body and `curl: HTTP <status>, <bytes> bytes`.
Non-text bytes are displayed as dots and lines wrap to its 64-column buffer.
Requests run synchronously in the terminal process; sleeping network waits allow
other processes and the compositor to run. The terminal redraws the body when the
request returns. Input is bounded to 512 bytes.

The separate [CURL application](../user_programs/curl/src/main.rs), installed as
`/BIN/CURL.BIN`, uses the same SDK. Launch CURL from the desktop and enter a URL
at `curl>`. An empty line exits. It writes decoded bytes to the application
console. It prompts because the current spawn ABI has no argv support.

The WM discovers up to 16 installed launchers, independently of its existing
six-window backing limit. CURL is installed alphabetically before Forge.

## Library

Applications can stream a body without storing the whole response:

```rust,no_run
use userlib::{http, sys};

let response = http::get("http://example.com/", |bytes| {
    if sys::console_write(bytes) == bytes.len() {
        Ok(())
    } else {
        Err(http::Error::Output)
    }
})?;
// response.status is the final HTTP status; response.bytes is decoded body size.
```

`get_with(url, Options, callback)` additionally configures the DNS server/port,
per-connection deadline, redirect limit, maximum decoded body size and optional
`tls_ca_file` (an additional DER CA, never a verification bypass). Enable the
`tls` feature on the `userlib` dependency for HTTPS; Portal, CURL and ZNETTEST do so. HTTP error
statuses such as 404 remain responses; protocol, DNS, socket, timeout and callback
failures are typed errors. Already streamed bytes are not rolled back on failure.
The callback may stop a transfer by returning an error. It should not start a
nested request on the same small userspace stack.

The standalone protocol crate accepts a `Transport` implementation. Host tests
supply sockets with short writes, fragmented receives and controlled time; guest
applications use the SDK adapter. Large parser phases explicitly remain separate
calls to bound stack use. User stacks are now 4 MiB to accommodate larger protocol parsing and application work.
Plain HTTP needs no allocator; TLS-enabled programs reserve a private 16 MiB heap.

## Implemented protocol and limits

- ASCII `http://` and `https://` URLs with DNS names or numeric IPv4, optional nonzero port,
  path/query and discarded fragments. Host headers include non-default ports.
  Credentials, IPv6 literals, control bytes and malformed authorities are rejected.
- Recursive UDP DNS queries, transaction/question/source matching, compressed names
  and CNAME chains with an A record in the answer. Default resolver is QEMU
  `10.0.2.3:53`. Up to three attempts, separated by 18 PIT ticks, within 54 ticks
  or the configured shorter deadline. Replies marked truncated or with DNS errors
  fail explicitly; DNS-over-TCP, IPv6/AAAA, caching and CNAME-only follow-up queries
  are not implemented. IDs derive from ticks and socket handles, not secure entropy.
- HTTP/1.1 GET with `Host`, `Connection: close`, and `Accept-Encoding: identity`.
  The client handles partial writes and HTTP/1.0 or HTTP/1.1 responses, bounded
  informational responses, Content-Length, chunked bodies/extensions/trailers,
  close-delimited bodies and no-body statuses 204/304. Chunk framing is removed
  before callbacks. Oversized/ambiguous/truncated framing returns an error.
- Redirects 301/302/303/307/308 follow absolute, scheme-relative, root-relative,
  query-relative or directory-relative HTTP/HTTPS locations, up to five by default.
  Redirects may change host and port. Redirect bodies are discarded. HTTP to HTTPS
  is allowed; HTTPS to HTTP fails with `InsecureRedirect` before opening the target.
  Redirects can reach other addresses accessible to the application.
- Default connection deadline is 180 PIT ticks (about 9.9 seconds), including
  connect, TLS handshake, writes and response reading. Every redirect gets a new deadline. A
  callback's execution time belongs to the application. Maximum decoded body is
  256 MiB by default; buffering is bounded independently of body size.
- Hostname limit 253 bytes, request target/redirect Location 16 KiB, assembled
  request 32 KiB, header/chunk/trailer line 32 KiB, total response headers
  256 KiB and trailers 256 KiB.
  Bodies stream in chunks of at most 1024 bytes. TLS has additional bounded
  record buffers and a private allocator; see [TLS architecture](tls.md).
- Duplicate Content-Length, TE plus Content-Length, unsupported transfer/content
  encodings, upgrades and obsolete folded headers are rejected. No decompression,
  authentication, cookies, POST/upload, proxy, HTTP/2 or HTTP/3.
- Verified TLS 1.3 for HTTPS: hostname, certificate chain, validity dates, signatures
  and server usage checks; SNI and HTTP/1.1 ALPN. No insecure mode or plaintext fallback.

HTTP framing references [RFC 9112](https://www.rfc-editor.org/rfc/rfc9112.html);
DNS wire format references [RFC 1035](https://www.rfc-editor.org/rfc/rfc1035).
This is a bounded GET client, not full curl compatibility. Public success depends
on the host's connectivity, DNS and the target supporting IPv4 and our TLS profile.

## Memory and size policy

[Exported limits](../crates/http/src/limits.rs) define the parser's bounded storage.
`Options::max_body` lets an application change the streamed body cap independently.
A 256 MiB response does not allocate a 256 MiB body buffer: callbacks receive at
most 1024 bytes at a time. Headers and chunk/trailer metadata use bounded 32 KiB
stack buffers. The 4 MiB user stack leaves room for larger redirect/request assembly
and TLS call depth. App input remains limited to 512 typed bytes; followed redirect
URLs can be longer. TLS record limits remain those required by the protocol.

The desktop and active E2E launchers provision 1 GiB of guest RAM, a 64 MiB kernel
heap, and 16 MiB private heaps for TLS-enabled apps. These heaps/stacks are eagerly
mapped fixed capacities, without demand paging or Linux-style virtual-memory growth.
Kernel stacks remain 16 KiB. Body/URL/header limits still reject oversized input
and close sockets; certificate checking and HTTPS downgrade protection still apply.
See [expanded-limit validation](http-limits-validation.md) for GitHub reproduction,
new boundaries, memory allocation and streaming tests.

## Verification

Build all apps and a fresh disk, then execute:

```bash
./tools/build-grub-disk.sh
python3 -B e2e/http.py
python3 -B e2e/http.py --site http://example.com/
```

Recorded results and remaining gaps: [HTTP validation](http-validation.md).
TCP receive-window measurements and optimization: [network performance](network-performance.md).

The fixture is an independent host TCP/DNS implementation. The test boots a private
disk copy, runs Portal curl and the standalone CURL app, then drives `ZNETTEST`
through real socket syscalls for DNS, content length, chunked/EOF framing,
redirects, truncated input, a 20,000-byte streamed body and repeated cleanup.
It preserves serial and framebuffer evidence in `target/http/`, terminates QEMU
and closes fixture sockets in cleanup. Public-site mode also requires an HTTP
status from the external URL. It does not substitute fixture success for Internet
access. Run the networking and desktop regressions in [testing](testing.md) too.

For pure host tests, run outside the root bare-metal Cargo configuration:

```bash
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path "$REPO/crates/http/Cargo.toml" --target x86_64-unknown-linux-gnu
```
