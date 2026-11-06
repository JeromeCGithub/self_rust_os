# Expanded native capabilities and validation

Implemented and tested on 2026-11-05, continuing the file-command and argument work
on `main` based at `3277448`. This expands the native kernel/SDK contract; it does
not provide Linux binary compatibility or a complete POSIX environment.

## Delivered behavior

| Previous limitation | Current behavior |
| --- | --- |
| Fixed 1 MiB SDK heap, 16 MiB with HTTPS | Default SDK allocator grows through `brk`, reuses freed blocks and handles failed growth without corrupting existing mappings. Roughly 12 GiB of virtual growth space, bounded by available physical RAM. |
| About 40 MiB for loaded image/BSS | Image arena extends from 4 MiB to 4 GiB; compact ELF BSS is zero-filled into private physical pages. A 64 MiB BSS probe runs successfully. |
| 16 descriptors, 1 MiB writes, full-file rewrite | Growable table supports 1024 regular descriptors. Offset writes touch affected data clusters, allocate extensions first and zero EOF gaps. FAT32 allows up to `u32::MAX` file bytes, subject to disk/resources. Transfers may be short and are capped at 64 KiB. |
| 8.3-only names | VFAT long names up to 255 UTF-16 units with unique compatible aliases, validated sequence/checksum decoding, long-name listing and UTF-8 paths shorter than 4096 bytes. |
| No general argv/environment | Native syscall 30 passes separate bounded argv/envp vectors. SDK supports inherited/explicit environments; Forge implements export, unset and env. Legacy launch syscalls remain available. |
| No hardware FP/compiler TLS | Per-thread x87/MMX/SSE2 state and FS base preservation; static local-exec PT_TLS initializes independent TDATA/TBSS for initial and worker threads. Linux ARCH_SET_FS/ARCH_GET_FS operation values are supported by native syscall 32. |

The kernel owns mechanisms, pointer validation and resource lifetimes. Forge owns
command parsing, quoting, executable selection, `cd` and environment policy.
Portal remains a terminal. New native numbers are 30 (launch vectors), 31 (`brk`),
32 (`arch_prctl` subset), and 33 (full-name directory records). Existing numbers
are preserved. See [ABI](syscalls.md), [layout](architecture.md),
[threading](threading.md) and [shell commands](terminal-shell.md).

## Observed red results

Before implementation, the corresponding executable regressions failed:

- Descriptor probe could not open the 17th regular descriptor.
- Streaming file probe failed growing beyond 1 MiB.
- SDK `try_reserve_exact(32 MiB)` failed against fixed heap storage.
- Initial executable with 64 MiB BSS was rejected by the loader.
- General argument/environment launch returned unknown-syscall error for 30.
- Long-name mkdir failed; the previous component parser accepted only 8.3.
- SSE register probe faulted on CPU execution before enablement/context support.
- Compiler TLS ELF was rejected as unsupported PT_TLS. The later FS-base boundary
  regression independently failed with unknown syscall 32 before that operation
  was added.

Red logs were retained during implementation as `/tmp/rustos-cap-CASE-red.log`
(including `arch`). These observations precede the final checks below; compilation
errors from unsupported harness filtering were not counted as behavioral reds.

Host packaging tests also exposed two stale assumptions: Forge was required to
reserve a 16 MiB BSS heap, and a shared-page metadata test assumed Spin would
always fit one read-only page. Tests now assert that Forge has no fixed BSS heap,
that the explicit 64 MiB BSS probe stays compact on disk, and that a controlled
shared-page ELF fixture retains the original 4096-byte permission-boundary result.

The HTTPS compatibility check additionally exposed its historical exhaustion
trigger: a 32 MiB allocation succeeded and the child returned 1 instead of the
expected allocation-failure exit code 12. Its request now exceeds the explicit
1 GiB guest RAM fixture (2 GiB); the original exit-code/socket-cleanup assertions
remain unchanged. The dedicated heap probe independently proves 32 MiB success.

## Checks executed

All runtime checks used freshly generated userspace binaries/metadata and a GRUB
disk at `target/capabilities.img`. Runners boot private writable copies and reap
their QEMU process. No host device or interactive VM disk was formatted.

| Check | Observed result |
| --- | --- |
| `CARGO_NET_OFFLINE=true ./tools/build-grub-disk.sh target/capabilities.img` | All apps rebuilt, release GRUB kernel and disposable disk built. |
| `CARGO_NET_OFFLINE=true cargo build` | Default boot path built. Debug GRUB path also built during implementation. |
| `CARGO_NET_OFFLINE=true cargo test --lib` | 90 cases passed, including range-write sector counters, disk-full extension rollback, long names/alias collisions/deletion, replacement rollback and user-copy validation. |
| `CARGO_NET_OFFLINE=true cargo test --test threads --test display_mapping --test stdio --test userspace` | All four harnesses passed (10 reported cases): mapping/isolation, repeated heap grow/shrink and frame recovery, lifecycle, stdio and display ownership. Constants-only userspace cases do not prove Ring 3 execution. |
| `./e2e/fat32.sh` | Disposable FAT32 fixture integration passed, including multi-cluster file round trip and deletion. |
| Host `process-abi`, `exec-image` tests | 5 and 6 passed respectively: legacy/new startup layout, limits, ELF malformed inputs, PT_TLS and segment bounds. Run from `/tmp` with explicit host target. |
| `python3 -B -m unittest discover -s tools/tests -p 'test_*.py'` | 37 passed: metadata, compact image output, packaging, docs checker and desktop runner. |
| `python3 -B tools/check_docs.py` and `git diff --check` | Passed. |
| Standalone LFN, Forge parser and editor model tests | 2, 2 and 4 passed respectively. |
| Capability cases `fds`, `files`, `heap`, `image`, `env`, `names` on 1 CPU | All passed through real Ring 3 syscalls; details below. |
| Capability cases `fpu`, `tls` on 1, 8 and 32 CPUs | All six runs passed. |
| `e2e/arguments.py`, `e2e/file_io.py` on current disk, 1 CPU | Passed: maximum 4096 argv entries, malformed legacy records, connected launch, cwd, shared offsets, quota side effects and exit/kill cleanup. |
| `RUSTOS_DESKTOP_CASE=capabilities RUSTOS_CPUS=8 python3 -B e2e/desktop.py` | Passed: export/inheritance/unset, quoted arguments and syntax errors, Hello window, surviving shell, quoted long directory, long filename write/list/cat and cd. |
| `RUSTOS_CPUS=8 python3 -B e2e/tls.py` | Local independent HTTPS fixtures passed: certificate/RSA/hostname/date/trust rejection, redirects, streaming beyond 16 MiB, allocation failure and cleanup. |

For all Python commands above, set `RUSTOS_TEST_DISK=target/capabilities.img`.
Focused cases run as `python3 -B e2e/capabilities.py --case CASE` with
`RUSTOS_CPUS=N`. Final command logs are `/tmp/rustos-cap-final-*.log`; runtime
serial diagnostics are retained under `target/cap-CASE/`, `target/file-io/`,
`target/arguments/`, `target/desktop/` and `target/tls-verified/` as applicable.
Later FP/TLS CPU runs replace that case's earlier serial file.

The heap probe repeatedly allocates/reuses 32 MiB, rejects a 2 GiB allocation in
1 GiB RAM, verifies unchanged break/frame count and rejects copying to rolled-back
pages. The file probe streams 4 MiB, patches across a sector boundary and verifies
zero-filled EOF gaps. An in-memory device records sector writes to prove an
in-place patch does not rewrite unaffected file data. Disk-full extension failure
preserves bytes and free-cluster accounting.

The name probe reads an independent mtools-created Unicode/spaced filename,
creates Unicode long names and a maximum-length component, and forces directory
cluster growth with 40 colliding-prefix aliases. After QEMU stops, host `mtype`
reads the guest-created Unicode file and verifies exact contents. The environment
probe checks explicit/inherited values, parent/child mutation isolation, a 16 KiB
value, malformed counts, unmapped request/vector pointers, wrong wire size and
SDK key validation. Full-name listing also rejects insufficient, overflowing and
unmapped output ranges without modifying a sentinel destination. These extra
boundary cases were rerun only for `env` and `names` after extending the probe.

FPU assembly keeps x87, MXCSR, XMM0 and XMM15 sentinels live across thousands of
yields in eight workers and independent child processes. Invalid-opcode failure
stops its child with status 6 while the parent survives. Compiler TLS tests
initialized/zeroed storage across eight workers, repeated join/reuse and stable
frame accounting, FS-base validation and actual f64 computation.

[Desktop evidence](screenshots/desktop-capabilities.png) shows Forge's export and
quoted launch, syntax-error recovery and the Hello window. Exact argv/environment
and disk results are asserted from execution; a screenshot alone cannot prove
those values. The screenshot was inspected from this tested build.

## Remaining limits and unverified checks

- This remains the native `int 0x80` ABI with native errors. There is no Linux ELF
  compatibility, auxiliary vector, dynamic linker, fork/exec, general pipes or
  complete permission model. Paths/arguments are UTF-8 rather than arbitrary bytes.
- Memory is eagerly mapped, without demand paging, swap, anonymous mmap or Linux
  overcommit. The kernel heap remains 64 MiB. Address arenas, physical RAM and
  kernel staging allocations bound loads and growth; the 4 GiB image limit is
  not a promise that a 1 GiB guest can load an image of that size.
- File tables allow 1024 regular descriptors plus separate standard streams;
  rlimit configuration is absent. FAT32 inherently bounds files to 4 GiB minus
  one byte. Offset writes still traverse FAT metadata and use serialized PIO;
  hardware failure can leave partial in-place data. No journaling/fsync or power
  loss guarantee is added. Whole-file replacement retains its separate staging
  and temporary-space costs.
- Startup allows 4096 arguments and 4096 environment entries with a combined
  128 KiB NUL-inclusive string budget. Legacy syscall 29 keeps its smaller limits.
- FP support covers the x86-64 x87/MMX/SSE2 baseline through FXSAVE64, not AVX/XSAVE.
  Compiler TLS is static local-exec only, at most 1 MiB per template, alignment
  at most 4096, with no dynamic module TLS or thread-local destructors.
- The existing debug heap stress timeout remains documented in
  [file-command evidence](file-commands-validation.md#screenshots-and-limits).
  It was reproduced on unchanged main in the preceding work, not rerun for this
  capability change. The focused rollback/resource tests above passed. The
  bootloader dependency still warns about unused `rustc-abi` target JSON fields.
- Manual host GUI capture/input and physical hardware remain unverified. QMP
  input proves guest interaction on the tested QEMU platform. The full unrelated
  desktop/network suite was not repeated; the relevant cases above were selected.

## Interface references

The static thread-pointer model follows the
[Linux x86-64 FS/GS explanation](https://www.kernel.org/doc/html/next/x86/x86_64/fsgs.html)
and the x86-64 variant-II layout in
[ELF TLS](https://www.uclibc.org/docs/tls.pdf). Kernel code remains soft-float,
consistent with the responsibility described by the
[Linux kernel floating-point API](https://kernel.org/doc/html/next/core-api/floating-point.html).
VFAT entry sequencing/checksums use the
[Microsoft FAT format specification](https://www.cs.fsu.edu/~cop4610t/assignments/project3/spec/fatspec.pdf)
and [UEFI FAT long-name format](https://uefi.org/specs/UEFI/2.10/13_Protocols_Media_Access.html).
These references guide the mechanisms; executed tests establish this OS's behavior.
