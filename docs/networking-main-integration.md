# Networking and native desktop integration

PR #36 incorporates `main` at `d9a9613` using a merge commit. Both the native
UI/desktop architecture and Ethernet/HTTP/TLS work are retained. This report
supersedes the earlier draft PR's unresolved integration status; milestone
validation reports still describe their original builds.

## Preserved features and conflict decisions

- Keep main's compositor split across `wm/{session,display,window,glass}`, its
  separate DE process, full-resolution space wallpaper, work-area negotiation,
  desktop role enforcement and compositor survival after DE exit.
- Keep the native window SDK, glass/rounded rendering, typed views/styles,
  `rsx!` components, widget state/capture and frame-copy acknowledgements. Portal
  retains native canvas rendering and now also runs ping and HTTP/HTTPS curl.
- Keep main's common Cargo application root and ordinary `fn main()` convention.
  CURL and ZNETTEST are migrated to that convention. All apps/binaries/metadata
  are rebuilt together; generated files are never resolved by hand.
- Application discovery stays in the DE, with its capacity increased from six
  to sixteen so CURL and ZNETTEST remain discoverable. The compositor keeps its
  existing six-window limit. No launcher/wallpaper policy moves into the kernel.
- Keep both sides of `display_mapping`: message authentication, queue capacity,
  foreign-client rejection and publication checks, plus complete stack mapping,
  unmapped guards, zeroing, isolation and repeated spawn/reap coverage.
- Preserve the dedicated kernel driver tree, process-owned socket ABI, full
  page-permission checks at network/platform boundaries, Rustls HTTPS, larger
  HTTP/memory limits and receive-window optimization.

## Explicit ABI and virtual-layout migration

Main's existing scalar message syscalls **20 and 21 remain unchanged**.
Networking ABI version **2** uses **64–70**; hardware randomness and UTC use
**80 and 81**. The unmerged network version 1 used 20–26 and platform 27/28.
Records and error encodings remain unchanged, but old networking binaries must
be rebuilt with the new kernel/SDK. The lower range remains available to core
interfaces, including the separate terminal/standard-I/O work. Dispatcher, shared
constants, typed wrappers, raw guest probes, docs and tests move together.

The native compositor's flat image is approximately 26 MiB, which did not fit
below the previous 4 MiB stack placement. The existing private region remains
`0x400000..0x4000000`; mappings now use:

| Region | Address/capacity |
| --- | --- |
| Program base | `0x400000` |
| User stack | `0x2c00000..0x3000000`, 4 MiB |
| Private surface / compositor framebuffer | `0x3100000` |
| Private region end | `0x4000000`, exclusive |

This leaves room for the native compositor and preserves stack/surface gaps.
The SDK framebuffer constant moves with the kernel; all native and legacy apps
are rebuilt. Kernel/user heap capacities stay 64/16 MiB and QEMU RAM stays 1 GiB.
The toolchain stays pinned to `nightly-2025-01-30`.

## Regression-driven fixes

- `/tmp/rustos-merge-abi-red.log`: the new collision test found 20–28 occupying
  the established/reserved core range. It passes after migration; runtime native
  IPC and network clients exercise the actual dispatcher together.
- `/tmp/rustos-merge-layout-red.log`: the strengthened compositor-sized image
  failed with `user binary overlaps stack`. The 30 MiB image now maps, preserves
  its final byte, and passes the combined messaging/memory-isolation harness.
- `/tmp/rustos-merge-image-red.log`: explicit integration-image selection was
  ignored. QEMU suites now accept `RUSTOS_TEST_DISK` and boot private copies, so
  integration builds never need to replace an interactive VM's development disk.
- The merged glass test initially observed unchanged hover/pressed pixels at
  its existing sample time. Pristine main was separately archived, built and
  tested successfully. Diagnostic sampling showed delayed pressed rendering,
  rather than a lost button event. Clicking an already focused top window
  unnecessarily recomposed the scene before its client's frame could arrive.
  Retain the scene until actual surface damage; still invalidate on restack,
  focus changes, legacy close, movement and lower-layer damage. The original
  glass assertion and waits remain intact; two full reruns passed after the fix.

The inherited space wallpaper is P6 raw RGB data. An explicit Git binary attribute
prevents text normalization and meaningless pixel-byte whitespace diagnostics;
the asset bytes remain identical to main. Its Git-attribute regression first
failed with `unspecified` and now passes.

The final HTTPS stress run exposed a rejection-path bug: unbuffered Rustls input
was left in place after an error, so trying to close reparsed the rejected flight
before transmitting its queued fatal alert. This could leave the independent
server waiting and make a subsequent rejection appear as unexpected EOF. The new
host regression in `/tmp/rustos-merge-alert-red.log` first failed because no alert
reached the peer. The wrapper now honors consumed prefixes, sends the terminal
alert without reparsing the failed flight, rejects further application I/O and
makes fatal close idempotent. All seven TLS host cases pass; original guest
certificate expectations are retained and include fixture context on failure.

## Reproduction and verification

Use a distinct disk path while another VM is running:

```bash
./tools/build-grub-disk.sh target/merged-main.img
RUSTOS_TEST_DISK=target/merged-main.img python3 -B e2e/desktop.py
RUSTOS_TEST_DISK=target/merged-main.img python3 -B e2e/glass_ui.py
RUSTOS_TEST_DISK=target/merged-main.img python3 -B e2e/tls.py --site https://github.com/
```

Kernel builds, compilation checks, 71 kernel runtime cases, complete selected
integration executables, five release heap cases and ATA/FAT32 fixture suites
passed during this integration. Locked protocol host suites passed, as did all
18 native UI/input/protocol/RSX cases, 6 glass cases and 4 damage cases. The Python
tooling suite has 24 cases including ABI collision and image-selection regressions.

Runtime results and screenshots are refreshed from the combined native build.
The full desktop suite retains DE/compositor separation, exact wallpaper pixels,
edge coordinates, commands, concurrent apps, focus/drag/close and DE-exit isolation.
The glass suite retains negotiated geometry, role validation, rounded/glass/text
pixels, pressed feedback, component state, backdrop resampling, capture/cancellation,
focus and live lower-layer damage. Networking checks retain actual guest HTTP/TLS,
verified public GitHub, large headers and 17 MiB streaming, entropy/clock refusal,
ICMP packet assertions, no-NIC recovery, ownership/cleanup and receive throughput.
Exact local commands/results are recorded under `/tmp/rustos-merge-*.log`.

The debug heap stress case still reaches its unchanged 60-second timeout; the
release suite passes. No assertion or timeout was weakened. Known process frame/
kernel-stack leaks, experimental RustCrypto provider status and unperformed manual
host GUI/physical-NIC checks remain as documented in [testing](testing.md) and
[TLS](tls.md). Successful guest QMP tests do not establish host pointer capture.


Final post-fix runtime matrix (`/tmp/rustos-merge-complete-results.txt`) passed:
TLS/GitHub twice, missing RNG, invalid clock, HTTP, complete desktop, complete glass,
CPU/input budgets, socket ABI/lifecycle, packet-verified public ping, no NIC and
throughput. The final TLS suite has seven cases, including terminal alert delivery
and repeated-close behavior. The original failed runs remain in the local logs;
no certificate, visual-feedback, timing or cleanup expectation was loosened.
The merged release-image profile check and scalar instruction audit also passed.
