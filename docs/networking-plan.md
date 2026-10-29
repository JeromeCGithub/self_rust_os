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

Next steps toward a general userspace HTTP client:

1. Implement DNS resolution over the UDP API, with request matching, bounded
   retries, malformed/truncated reply tests and optional TCP fallback.
2. Build a userspace HTTP library and command with URL parsing, explicit connect
   and total deadlines, short reads/writes, bounded header/body storage,
   Content-Length/chunked framing and redirect policy. Keep HTTP out of Ring 0.
3. Add graceful TCP shutdown and a socket wait interface with cancellation and
   process cleanup tests. Introduce a kernel worker when service must continue
   independently of polling callers; preserve bounded IRQ work and lock order.
4. Add TLS only with tested cryptographic entropy, certificate validation and
   appropriate time support. A successful plain HTTP probe is not HTTPS support.
5. Add DHCP, additional NICs, IPv6 or server sockets as separate tested milestones.

Every behavior change continues to require red/green tests at its failure layer,
including real Ring 3/QEMU traffic for new syscalls or hardware behavior. Pin any
future toolchain change to an exact dated release and verify all affected kernel,
userspace, boot and runtime paths before accepting it.
