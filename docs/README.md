# Project documentation

Read these before changing the OS. The source and reproducible tests determine
current behavior; architecture targets are not evidence of implemented features.

| Document | Purpose |
| --- | --- |
| [AI contribution instructions](../AGENTS.md) | Mandatory TDD, verification, safety, and delivery rules |
| [Architecture](architecture.md) | Ring 0/Ring 3 ownership, driver hierarchy, repository map, lifecycle, and limitations |
| [Development](development.md) | Toolchain, build artifacts, red/green/refactor workflow, PR expectations |
| [Testing](testing.md) | Change-to-test matrix, commands, evidence requirements, baseline gaps |
| [Performance validation](performance.md) | 1080p optimization measurements, tests, and remaining limits |
| [Syscall ABI](syscalls.md) | Current interface between kernel and userspace |
| [Networking architecture](networking.md) | Ethernet, ARP/IPv4, ICMP/UDP/TCP, socket ABI and userspace clients |
| [Userspace HTTP and curl](http.md) | Streaming HTTP SDK, DNS, curl commands and protocol limits |
| [Userspace TLS](tls.md) | Rustls integration, HTTPS SDK, entropy/time boundary and limits |
| [Expanded HTTP and memory limits](http-limits-validation.md) | GitHub header reproduction, larger budgets, streaming and allocation evidence |
| [TLS validation](tls-validation.md) | Certificate failures, public HTTPS and runtime evidence |
| [HTTP validation](http-validation.md) | HTTP red/green evidence, public request and remaining limits |
| [Network performance](network-performance.md) | TCP window optimization, DMA, measurements and future event delivery |
| [Networking validation](networking-validation.md) | Red/green evidence, packet captures and remaining limits |
| [Ethernet networking plan](networking-plan.md) | Delivered ping/client sockets and roadmap toward DNS, HTTP and TLS |
| [GRUB boot](grub-boot.md) | Current disk layout and boot sequence |

Scoped instructions: [kernel](../src/AGENTS.md), [applications/SDK](../user_programs/AGENTS.md),
[kernel tests](../tests/AGENTS.md), and [QEMU E2E](../e2e/AGENTS.md).

[Filesystem review](arch/fs-hierarchy-review.md) is historical context from an older
PR. Re-check its claims against the current implementation before applying them.

[Claude entry point](../CLAUDE.md) and [Copilot instructions](../.github/copilot-instructions.md)
point back to the same policy. The [PR template](../.github/pull_request_template.md)
requests red/green evidence; [documentation CI](../.github/workflows/documentation.yml)
runs the checker and its regression tests. Instructions express required behavior;
CI checks links and entry points, not whether an agent truly followed TDD.
