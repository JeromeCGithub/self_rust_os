# Interactive SMP latency

The reported workload was `RUSTOS_CPUS=16 cargo run`: two terminals, slow shell
startup and delayed keyboard/pointer response. The fix keeps shell and rendering
policy in Ring 3 and changes the kernel mechanisms that those applications use.

## Changes and boundaries

- [Readiness waits](../src/scheduler/events.rs), additive syscall 109 and typed
  [SDK wrappers](../user_programs/userlib/src/sys.rs) atomically check/park on
  mailbox, connected-I/O read/write, focused-keyboard and display-owner pointer readiness.
  Complete pointer samples publish a revision; a mouse read consumes that revision.
  IRQs issue at most one idle-CPU doorbell, without scheduling or allocating. Enqueue,
  dequeue and peer exit wake the relevant waiters. EOF is readiness; consumers
  recheck I/O. No user pointer survives parking.
- [Idle CPU selection](../src/scheduler/placement.rs) assigns at most one idle
  eligible CPU to each ready context. One runnable thread no longer wakes all
  16 idle CPUs. Stack handoff and mapping transitions retain the entry gate.
- [Portal](../user_programs/portal/src/main.rs) uses the public native thread SDK:
  one renderer and one I/O worker. A mutex protects text snapshots; no screen
  lock spans painting or compositor acknowledgement. Revision publication and
  process-private word wakeups notify the renderer. Text is sampled after the
  preceding frame acknowledgement, so incoming output coalesces into a fresh frame. Forge waits for input;
  active asynchronous ping retains its bounded deadline service.
- [Executable validation](../crates/exec-image/src/lib.rs) accepts static x86-64
  ELF and legacy flat images. The [mapper](../src/userspace/process.rs) copies
  file bytes, zeroes BSS/padding, uses the declared entry address and unions
  permissions for distinct byte ranges sharing a page (including GOT). Parsing
  rejects malformed/overlapping/out-of-range mappings and unsupported dynamic
  linking or compiler TLS before allocation. Kernel/frame ownership remains
  transactional. ELF file/memory sizes follow the
  [ELF program-loading contract](https://gabi.xinuos.com/elf/07-pheader.html).
- The compositor redraws an isolated content update without rebuilding disjoint
  windows outside the blur sampling margin. Overlap, nearby sampled layers,
  background, focus and geometry changes keep the complete
  bottom-to-top path. Single-buffer RELEASE follows presentation, pacing the next
  client snapshot. Unchanged backdrops are cached for the isolated-content path;
  dependency changes rebuild them. This reserves about 6.3 MiB in the compositor
  for six bounded slots, including pixels hidden by opaque foreground. Glass
  interpolation is inlined without changing its arithmetic. Cursor refreshes retain
  sampled button transitions in a bounded Ring 3 queue, preventing a click during
  rendering from disappearing before focus policy runs. Motion still coalesces;
  overflow retains the oldest edges and the latest state, not unlimited history.
- The builder emits stripped ELF under the existing `.bin` filenames, with
  regenerated `_meta.rs`. Forge's stored file shrank from 17,514,546 to 741,072
  bytes; its runtime heap size is unchanged. BSS is initialized in private RAM.
- FAT chain reads retain one metadata sector with write invalidation; ATA uses
  bounded multi-sector commands and exact 512-byte string-I/O transfers. Drivers
  remain under `src/drivers/`; filesystem/device locks retain resource ownership.

## Red evidence

The initial red observations were recorded during the October 31 work before
their corresponding implementations. November 4 additionally exposed the backdrop
sampling-margin and click-during-rendering failures listed below. The resumed
checks use rebuilt code; earlier measurements remain separate from fresh runs:

| Regression | Observed missing behavior |
| --- | --- |
| Two terminals, 16 CPUs | Launches 1,591/1,539 ms; command drain 102/133 ms, exceeding the initial 100 ms bound |
| Stronger launch/input bound | Event-only build still took 1,459/1,447 ms; second command drain 112 ms |
| Scheduler idle targets | One ready context selected 16 CPUs instead of one |
| Readiness context test | Writable live endpoint returned error rather than readiness bit 4 |
| FAT metadata read count | A 100 KiB sequential file reread the same FAT metadata 49 times |
| ELF parser | Valid compact static image rejected |
| Actual ELF mapping | User code contained `[127,69,76,70]` (ELF magic) instead of `[144,195,144,144]` |
| Executable artifacts | Builder produced raw flat images with materialized heaps |
| Shared GOT layout | Valid page-sharing load segments rejected; Spin metadata missed the first writable page |
| Pointer publication | A complete input packet did not advance readiness; partial packets must not publish |
| Two-window visible output | Serial commands completed while the displayed text still contained an older partial command |
| Click during composition | The 1-CPU Spin load test lost terminal focus and failed `focused input stalled under CPU load`; three host cases then failed for edge ordering, motion and overflow |
| Backdrop dependency margin | Nearby non-overlapping rectangles were incorrectly classified as independent despite glass sampling beyond the paint edge |
| Desktop close/reap | Automatic owned-child retirement omitted the existing reaping trace; it now uses the standard retirement path |

The final keyboard timer starts **before Enter injection**. The original baseline
measured drain after the final key's 25 ms injection delay, so the final check is
stricter. Launch timing includes the same 300 ms QMP click injection sequence.
No existing timeout or assertion was relaxed.

## Reproduction and acceptance

```bash
./tools/build-grub-disk.sh target/delivery.img
RUSTOS_TEST_DISK=target/delivery.img RUSTOS_CPUS=16 python3 -B e2e/desktop_parallel.py
RUSTOS_TEST_DISK=target/delivery.img python3 -B e2e/smp.py --cpus 1 2 8 16 32 --stress
```

The two-terminal test asserts independent focused command responses, visible text
in both surfaces, launch below 1 second, pointer response below 50 ms while idle or during
launch, and Enter-to-response below 75 ms. Each VM owns a disposable disk. Per-CPU
metrics, serial output and screenshots are saved under `target/desktop-parallel/`.
Performance runs must not compete with other test VMs.

Fresh isolated measurements on 2026-11-04, using headless QEMU/TCG and the
same 1 GiB/e1000 platform profile as the desktop runner:

| CPUs | First / second launch (ms) | Worst pointer (ms) | Worst Enter response (ms) |
| --- | --- | --- | --- |
| 1 | 322.1 / 322.8 | 29.9 | 25.9 |
| 8 | 335.4 / 346.7 | 19.1 | 5.7 |
| 16 | 324.6 / 336.8 | 19.1 | 5.6 |
| 32 | 328.2 / 326.8 | 17.7 | 5.6 |

The 16-CPU row uses a repeated run; both runs passed. Both responses also passed the unchanged 300 ms framebuffer visibility assertion.
The [16-CPU screenshot](screenshots/smp-parallel-terminals.png) shows the distinct
`echo first` and `echo second` results in two native Portal windows. It was captured
from that tested run; it supplements the assertions, not the timing evidence.
Measured disk SHA-256: `df09677f0d67e72cbe9c9ddd9432b825b72d4de547bb2e2891812da68cbc7a50`.

The resumed verification passed default/GRUB/release builds, `cargo test --no-run`,
85 kernel unit cases, nine selected integration executables (16 cases), three ATA
cases, two FAT32 cases, and five release heap cases. Host checks passed five ELF
parser cases, five damage cases, three pointer-input cases, six glass cases, two screen cases, 18 SDK UI cases,
35 Python tooling cases and all four native-thread compile/compile-fail probes.
The complete 16-CPU desktop and glass interaction suites passed, including actual
commands/file results, focus, drag/close, owned shell reaping, background changes and
live overlapping-layer damage. The 1/2/8/16/32 CPU stress matrix passed, including
an actual Ring 3 text write fault (`vector=14 error=0x7 address=0x400000`) and frame
recovery. The permission probe also ran with its updated executable in a separate
disposable image.

Networking checks passed the Ring 3 boundary/lifecycle/UDP/TCP probe, missing NIC,
48 packet-verified ping replies, unreachable-peer timeout, HTTP/CURL, verified TLS
including the public HTTPS case, and entropy/time failure profiles. The isolated 1/16-CPU desktop CPU/input-under-load
checks (including a repeated 1-CPU run after the sampled-button fix) and
16-CPU network throughput budget also passed. Current logs
are under `target/validation/`; failure and manual-check limits follow below.

Kernel tests additionally assert FIFO ordering, enqueue-before-park, parked-reader
and backpressured-writer wake return values, EOF, endpoint isolation and owner-death
cleanup. The `threads` integration checks actual mapped bytes, BSS/padding zeros,
declared entry, code permissions, shared GOT pages, parent isolation and repeated
frame recovery. SMP acceptance exercises native worker affinity/shared-memory
progress, blocking, joins/detaches, kill/reap, faults, TLB invalidation and physical
allocation exhaustion/rollback across the supported CPU profiles.

## Limits and remaining checks

User instructions execute on multiple CPUs, and Portal's I/O progresses independently
of rendering. The shared round-robin ready queue, entry gate and coarse service
locks still limit kernel throughput. This is not a fully preemptible kernel or a
claim that a sequential application automatically uses every core. ELF code pages
are read only when not shared with writable data; NX/complete W^X, dynamic linking,
compiler TLS and hardware FPU/SIMD context switching remain unimplemented.

The unchanged debug heap stress suite still times out at its existing 60-second
limit after the first four cases pass. The release suite passes. That failure was
also reproduced on the earlier baseline; its allocation count/timeout were not
changed or skipped. QEMU headless/QMP checks do not establish physical-hardware
performance or manual GTK/Wayland/X11 input behavior. Those host GUI checks remain
for the reported machine using `RUSTOS_CPUS=16 cargo run`.
