# Project documentation

Read these before changing the OS. The source and reproducible tests determine
current behavior; architecture targets are not evidence of implemented features.

| Document | Purpose |
| --- | --- |
| [AI contribution instructions](../AGENTS.md) | Mandatory TDD, verification, safety, and delivery rules |
| [Architecture](architecture.md) | Ring 0/Ring 3 ownership, driver hierarchy, repository map, lifecycle, and limitations |
| [Native threads and SMP](threading.md) | Current 1–32 CPU support, Ring 3 threading/synchronization APIs, ownership, limits and validation |
| [Interactive SMP latency](parallel-latency-validation.md) | Two terminal/shell pairs, event waits, compact loading and measured 16-CPU response |
| [Threading validation](threading-validation.md) | Executed red/green checks, baseline failures and verification limits |
| [Original threading proposal](threading-smp-plan.md) | Historical design baseline; consult the current threading guide for implemented behavior |
| [Development](development.md) | Toolchain, build artifacts, red/green/refactor workflow, PR expectations |
| [Testing](testing.md) | Change-to-test matrix, commands, evidence requirements, baseline gaps |
| [Performance validation](performance.md) | 1080p optimization measurements, tests, and remaining limits |
| [Syscall ABI](syscalls.md) | Current interface between kernel and userspace |
| [Native desktop/network integration](networking-main-integration.md) | Preserved main features, explicit ABI/layout migration and merged validation |
| [Networking architecture](networking.md) | Ethernet, ARP/IPv4, ICMP/UDP/TCP, socket ABI and userspace clients |
| [Userspace HTTP and curl](http.md) | Streaming HTTP SDK, DNS, curl commands and protocol limits |
| [Userspace TLS](tls.md) | Rustls integration, HTTPS SDK, entropy/time boundary and limits |
| [Expanded HTTP and memory limits](http-limits-validation.md) | GitHub header reproduction, larger budgets, streaming and allocation evidence |
| [TLS validation](tls-validation.md) | Certificate failures, public HTTPS and runtime evidence |
| [HTTP validation](http-validation.md) | HTTP red/green evidence, public request and remaining limits |
| [Network performance](network-performance.md) | TCP window optimization, DMA, measurements and future event delivery |
| [Networking validation](networking-validation.md) | Red/green evidence, packet captures and remaining limits |
| [Ethernet networking plan](networking-plan.md) | Delivered ping/client sockets and roadmap toward DNS, HTTP and TLS |
| [Complete browser roadmap](browser-support-plan.md) | Phased plan and acceptance gates for classic browsing, Google results, YouTube, Facebook and broader compatibility |
| [jbrowse scripting prototype](jbrowse-javascript.md) | Bounded inline JavaScript subset, typed DOM capabilities, engine research and limits |
| [Google startup and forms](jbrowse-google.md) | Automatic Google homepage, generic GET forms, charset decoding and the remaining results-page limitation |
| [jbrowse browser](jbrowse.md) | Native Ring 3 HTML/CSS browser, supported subset, limits and real-site testing |
| [Window and UI toolkit](ui.md) | Native window API, canvas/widgets, glass materials and protocol |
| [Expanded native capabilities](capabilities-validation.md) | Growable heap, larger images, offset writes, VFAT, environment and FP/compiler TLS evidence |
| [Program arguments](arguments-validation.md) | argc/argv startup ABI, shell quoting and focused validation |
| [File commands and editor validation](file-commands-validation.md) | cd/cat/mkdir/medit, descriptor ABI and executed red/green evidence |
| [Terminal and shell](terminal-shell.md) | Separate terminal presentation, shell execution and connected standard I/O |
| [Desktop environment](desktop-environment.md) | Separate DE/compositor processes, shell protocol and default space wallpaper |
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
