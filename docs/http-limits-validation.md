# Expanded HTTP and memory limit validation

Implemented on 2026-10-30 in the networking worktree, branch
`plan/qemu-ethernet-ping`. The reported URL was `https://github.com`.
The host's HTTP/1.1 response had 5,662 header bytes; its Content-Security-Policy
line alone was 4,214 bytes. The old 1,024-byte line limit rejected that header
before reading the body. A larger heap alone would not fix this parser limit.

## Delivered limits

| Resource | Previous TLS milestone | Current |
| --- | --- | --- |
| Individual HTTP status/header/chunk-extension/trailer line | 1 KiB | 32 KiB, excluding CRLF |
| Response header block / trailer block | 8 KiB each | 256 KiB each |
| Request target / redirect Location | 1 KiB | 16 KiB |
| Request / absolute redirect assembly | 1.5 KiB | 32 KiB |
| Default decoded response body | 16 MiB | 256 MiB, configurable through Options::max_body |
| Private heap in TLS-enabled apps | 1 MiB | 16 MiB |
| Private user stack | 128 KiB | 4 MiB |
| Kernel heap | 32 MiB | 64 MiB |
| Active desktop/E2E guest RAM | 256 MiB | 1 GiB; bootimage tests also explicitly use 1 GiB |

User stack top stays at `0x2000000`; bottom moves to `0x1c00000`. Kernel stacks
remain 16 KiB. The existing private region, framebuffer address, syscall numbers,
TLS record bounds, trust validation and pinned `nightly-2025-01-30` are unchanged.

The kernel maps stack pages and owns physical allocation; HTTP parsing and the
private allocation library remain in Ring 3. No new syscall was needed. Heaps and
stacks are fixed, eagerly mapped capacities, without demand paging or automatic
growth. The private free-list heap reuses freed allocations within the process;
existing process frame/kernel-stack reclamation gaps remain. More RAM does not
repair those leaks. Flat binaries still materialize BSS, making TLS-enabled images
roughly 17 MiB. The 128 MiB development disk still accommodates the installed apps.

HTTP continues streaming 1,024-byte body chunks, so a 256 MiB body does not require
a 256 MiB allocation. Stack metadata buffers and URL assembly have explicit caps.
The connection deadline remains configurable and its default is unchanged; a large
slow download may need a larger timeout supplied through `get_with`.

## Red tests and fixes

| Local log | Actual failure before the fix |
| --- | --- |
| `/tmp/rustos-limits-http-red.log` | Five new tests failed with TooLarge: long security headers, larger header blocks, 8 KiB redirects, long chunk metadata/trailers, and a streamed body beyond the previous cap |
| `/tmp/rustos-limits-stack-red.log` | Full 4 MiB writable stack mapping assertion failed on the old 128 KiB mapping |
| `/tmp/rustos-limits-kernel-heap-red.log` | Fallible 40 MiB allocation failed in the old 32 MiB kernel heap |
| `/tmp/rustos-limits-user-heap-red.log` | Actual Ring 3 fallible 8 MiB allocation failed in the old 1 MiB private heap |
| `/tmp/rustos-limits-platform-red.log` | Shared QEMU profile lacked the requested memory budget |

The existing oversized-header negative test now supplies a line above the new
32 KiB cap and retains its rejection and close assertions. New tests cover the
exact accepted line boundary and rejection one byte above it, excessive aggregate
headers, the full accepted 256 MiB body boundary, rejection one byte above that,
and existing caller-selected smaller body limits.

Expanding the kernel heap exposed a separate performance problem: legacy boot's
`BootInfoFrameAllocator` restarted the usable-frame iterator and used `nth(next)`
for every allocation. Initializing 64 MiB became quadratic and the mapping harness
timed out before its test cases. Whole usable regions are now skipped
arithmetically, matching the existing GRUB allocator strategy. The mapping harness
passes with the same 60-second timeout. New kernel cases cover reserved-region
skipping, ordered distinct frame addresses, empty maps and repeated exhaustion.
Neither frame ownership nor the existing batch rollback contract was weakened.

The socket probe initially treated `0x1000000` as unmapped; the expanded private
BSS heap deliberately maps that address. It now probes the actual lower stack
guard, retaining the exact Invalid assertion. The original failure is recorded
in `/tmp/rustos-limits-final-sockets.log`.

Desktop verification also found an interactive QEMU holding the development image
open. The original desktop harness tried to share its writable image, producing
QEMU's write-lock rejection followed by ConnectionRefusedError. The builder had
already overwritten that live development image before this was identified; this
was a workflow mistake, and the user's VM needs a restart to load a consistent
new kernel/application build. It was not terminated automatically. Desktop/CPU
harnesses now boot private disposable copies, matching the existing network/TLS
fixtures. Host tests cover fixture write isolation and bounded socket-readiness
retry/cleanup; existing startup deadlines remain unchanged. Their red evidence is
`/tmp/rustos-limits-disk-isolation-red.log` and `/tmp/rustos-limits-monitor-red.log`.

## Green evidence executed for this change

Host commands run from `/tmp` with exact nightly and explicit
`--target x86_64-unknown-linux-gnu`, using each crate's locked manifest:

- HTTP: two DNS cases plus twenty client cases, including a generated 256 MiB
  stream without constructing that body in host memory.
- TLS: six cases; net ABI two; net-stack five; ping three; platform ABI one.
- Complete Python tooling suite: twenty-one cases, including the QEMU memory profile.

Kernel/build checks:

- Default kernel, debug/release GRUB builds, all userspace through
  `tools/build-user.sh`, fresh GRUB disk, `cargo test --no-run`.
- `cargo test --lib`: 71 runtime cases; `cargo test --test display_mapping`: two
  cases, including full 4 MiB mapping, lower guard, zeroed independent stack
  endpoints and repeated process spawning/reaping.
- Basic boot, gfx, userspace, expected panic, expected stack overflow and
  keyboard/timer integrations passed.
- `cargo test --release --test heap_allocation`: five cases, including simultaneous
  40 MiB staging and 2 MiB overhead allocations.
- `bash e2e/ata.sh` and `bash e2e/fat32.sh`: disposable-fixture suites passed.

Runtime evidence from the rebuilt image:

- `python3 -B e2e/tls.py --site https://github.com/`: verified public HTTPS HTTP 200,
  plus independent OpenSSL fixtures. Public response size varies; the first run
  returned 530,078 bytes. Actual screenshot is linked below.
- The guest probe allocates, fills, validates and frees 8 MiB twice, checking heap
  use returns to baseline. It accepts a 20 KiB security header and roughly 180 KiB
  header block, an 8 KiB redirect with partial socket writes, 4 KiB chunk extensions
  and 20 KiB trailers. Excessive individual headers, aggregate headers and declared
  bodies are rejected before callbacks, releasing sockets.
- A separate verified HTTPS request streams exactly 17 MiB (17,825,792 bytes),
  checks byte count/checksum and allocator/socket recovery. Its explicit longer
  deadline is part of that new large-download test, not a relaxed existing timeout.
- The OOM child requests 32 MiB, exceeding the new 16 MiB heap. Its expected exit
  code 12 and parent/socket cleanup assertions remain unchanged.
- `e2e/tls.py --no-rng`, `e2e/tls.py --bad-clock` and `e2e/http.py` passed.
- `e2e/network.py --address 8.8.8.8` verified four replies on the wire;
  `--probe` passed real user-buffer, UDP/TCP, ownership and exit/kill cleanup checks;
  `--no-nic` reported absence and recovered the prompt.
- `e2e/net_perf.py --label expanded-limits --assert-fast`: 128 KiB transferred
  in 3 and 2 PIT ticks, retaining the 32 KiB TCP receive window.
- The complete `e2e/desktop.py` and `e2e/desktop_perf.py` passed on private disks
  while the interactive VM remained open. The CPU run measured idle 0.0%, idle
  terminal 3.5%, Snake/terminal 8.0% of a host core and 53 ms pointer latency under
  CPU load; its existing input/CPU limits were retained.
- Final Portal, CURL and ZNETTEST ELF disassembly contains no SIMD/MMX/x87 register
  instructions. Documentation links, tooling tests, shell syntax and diff whitespace
  checks passed.

[GitHub HTTPS screenshot](screenshots/github-curl.png) is captured from the actual
final tested guest after Portal publishes its response. Serial evidence lives in
`target/tls-verified/serial.log`; logs/images under `target/` remain untracked.
Local `/tmp/rustos-limits-*.log` files record commands, failures and subsequent passes.

## Remaining failures and limits

The debug heap stress case still times out at the unchanged 60-second limit.
Its loop uses HEAP_SIZE, so the expanded heap now exercises 67,108,864 allocations
instead of 33,554,432. The assertions were not reduced or skipped. The release
suite passes, and the ordinary kernel mapping/unit harnesses pass after fixing
frame allocation. The bootloader dependency still emits its known unused
`rustc-abi` field warning.

TLS's experimental crypto-provider status and protocol limits remain as documented
in [TLS](tls.md). No security audit, arbitrary-site compatibility or process frame
reclamation is claimed. Manual host GUI checks remain unperformed; QMP establishes
guest interaction only. Validation here describes the networking branch before integration with current main.
