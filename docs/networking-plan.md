# Networking roadmap

Current implementation and ABI: [networking architecture](networking.md).
Evidence and remaining verification gaps: [validation](networking-validation.md).

The original milestone was a Ring 3 `ping 8.8.8.8` over QEMU Ethernet. The
implementation now provides separate device, link, Internet, transport, syscall,
SDK and application layers, with outgoing TCP and connected UDP in addition to
ICMP. `nightly-2025-01-30` remains pinned; no toolchain migration was necessary.

| Milestone | Delivery |
| --- | --- |
| Explicit Ethernet backend | Shared tested QEMU e1000/user profile plus no-NIC mode |
| PCI, MMIO, DMA | Supported-device discovery, uncached supervisor BAR, bounded reset, reusable 32-entry RX/TX rings |
| ARP and routed IPv4 | Pinned smoltcp, static guest configuration, gateway resolution, checked protocol framing |
| Socket boundary | PID/generation handles, bounded polling, fallible buffers, mapping-aware copies, close/exit/kill cleanup |
| Userspace ping | Portal command with option parsing, nonce/payload/sequence checks, bounded retries and loss statistics |
| Client transports | Nonblocking UDP and outgoing TCP through open/connect/send/recv/close/poll/info syscalls |
| Application networking proof | Independent host fixture receives an HTTP/1.0 request from a real Ring 3 TCP client |

The [userspace HTTP SDK and curl](http.md) now deliver DNS A/CNAME resolution,
URL parsing, partial TCP writes, streaming Content-Length/chunked/EOF framing,
redirects, deadlines and socket cleanup. [TLS 1.3 and HTTPS](tls.md) now reuse
Rustls in userspace with certificate verification, hardware entropy and UTC time.
Next application networking work includes richer HTTP methods, TLS 1.2 compatibility
and DNS TCP fallback/AAAA/cache support. Background protocol service, server sockets and
graceful shutdown remain kernel/interface work.

Keep the existing dated toolchain pin. Pin any
future toolchain change to an exact dated release and verify all affected kernel,
userspace, boot and runtime paths before accepting it.
