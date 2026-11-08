# Initator M1: implementation and validation

This is the M1 record. [M2](initator-m2-progress.md) supersedes its compiled catalog
with [configured units](initator-units.md); the kernel supervision ABI is unchanged.

M1 provides an opt-in headless native PID 1 supervising two trusted foreground
services. It does not implement the later configuration/graph, IPC, login, user
manager or graphical-session milestones. See [the complete roadmap](init-system-plan.md).
Tool names are **Initator** (PID 1), **Nest** (future user manager), and **Spark**
(future control client). Glow and Flicker are trusted lifecycle fixtures; Zinit is
an excluded `.test-only` Ring 3 probe.

Implementation worktree: `/home/jay/personal/self_rust_os-initator`, branch
`feat/initator-m1`, based on main `167bde41e4752c185414f94d80e09e0f5b0a3007`.
Unrelated work in the original worktree was preserved. One PR is prepared per
milestone; no merge is authorized or performed.

## Behavior and boundary

- `RUSTOS_BOOT_PROFILE=native-init` builds `grub,native-init`, reserves PID 0,
  and launches `/BIN/INITATOR.SVC` as the first userspace process, PID 1.
  It has private RAM presentation pages, not the hardware display mapping.
- Native parent identity is installed before process publication. New scalar
  operations 35–37 query a complete identity, consume an owned child event, or
  terminate an owned child. The [ABI](syscalls.md#owned-child-supervision-version-1)
  uses explicit little-endian records, generation validation and whole-buffer
  usercopy before consuming status. Existing syscall numbers remain stable.
- Readiness bit 32 parks on durable child completion/adoption state under scheduler
  serialization. Live orphan notifications and already-dead orphan status go to
  init; terminal completion supersedes an unconsumed adoption event. Connected
  descendants still stop on owner death, without losing their native completion.
- Kernel process ownership checks apply to both old and new control operations in
  native mode. Oversized PID values and nonzero unused control arguments are
  rejected before truncation/side effects in both profiles. Restricted workers
  remain default-deny for new operations.
- `crates/service-core` owns fixed-size, allocation-free userspace policy: run
  identities, one process per run, stop suppression, failure classification,
  capped exponential backoff, rolling attempt windows and healthy resets.
  Timestamp history is fixed at 32 entries per service; larger bursts are rejected. The native
  adapter waits for child events or the nearest monotonic deadline.
- Native PID 1 exit, user fault or last-thread exit parks other CPUs through a
  lock-independent NMI path, attempts a serial diagnostic without waiting on its
  lock, and halts. Frames/device buffers remain owned. This is fail-stop, not init
  restart, graceful shutdown, durable storage or poweroff.
- Default desktop boot remains PID 0 WM → DE. Service `.SVC` executables are hidden
  from `.BIN` application discovery; ordinary applications/workers gain no restart
  policy. The graphical cutover belongs to M6.

## Red evidence recorded during this implementation

| Test/command | Observed missing behavior before implementation |
| --- | --- |
| Host `service-core` tests | Eight assertions failed against the initial inert supervisor: no launches and invalid limits accepted |
| `cargo test --test process_supervision` | Child bit returned error rather than parking; later valid consumption failed after invalid-buffer attempts; owned termination failed; fault was classified as explicit exit |
| `cargo test --test native_supervision` | Native profile did not reserve PID 0; ownership/orphan cases were then implemented and passed |
| `python3 -B -m unittest discover -s tools/tests -p test_init_profile.py` | Native profile was ignored; invalid profile reached the build |
| `e2e/init.py --case two-services` | Real GRUB boot created legacy PID 0 rather than native PID 1 |
| Same native test at 8 CPUs | Multi-syscall formatting interleaved Glow output with Initator's start record; assertion retained and output framing fixed |
| Host `io/record.rs` tests | Complete/exact-capacity records returned errors before bounded formatting was implemented |
| `e2e/init.py --case fatal-exit/fatal-fault/fatal-thread` | PID 1 died while Glow continued running; each test subsequently required all CPUs halted |
| `e2e/init.py --case abi` | A high-bit PID killed its low-32-bit target instead of returning an error |
| Rolling-window audit test | Fixed window reset admitted an extra restart while three attempts remained inside the preceding interval; replaced with bounded timestamp history |
| Strengthened native timing test | Tick-stamped backoff evidence was missing before timing records were added |

The initial standalone formatter harness first hit a visibility compile error;
that was corrected before recording its two behavioral red assertions. Missing
tools/compile errors are not counted as behavioral red evidence.

## Green checks actually executed

All results below were executed during M1 work, rather than inherited from old
validation documents. Relevant checks were rerun after behavior changes affecting
them; compilation is listed separately from runtime assertions.

- `cargo build`, `cargo build --features grub`,
  `cargo build --release --features grub`: passed.
- `cargo test --no-run`: all harnesses compiled; this is not a runtime test pass.
- `cargo test --lib`: 91 cases passed in the QEMU custom harness.
- `cargo test --test process_supervision --test native_supervision --test stdio --test threads --test display_mapping`:
  all six selected cases passed, including parent/orphan authority, consumption,
  typed exit/kill/fault, connected cleanup, private mappings, frame/kernel-heap
  recovery, and existing readiness/context behavior.
- `cargo test --test basic_boot --test userspace --test keyboard_timer`: passed.
  `userspace` remains a constants/mapping harness; actual Ring 3 proof is below.
- Host `crates/service-core`: eleven policy tests passed. Host `crates/process-abi`:
  seven existing and two new wire/error tests passed, including the expanded
  restricted-worker denial checks. Run manifests from `/tmp` with
  `+nightly-2025-01-30 --target x86_64-unknown-linux-gnu`.
- `rustc +nightly-2025-01-30 --edition=2021 --test user_programs/userlib/src/io/record.rs -o /tmp/initator-record-tests`
  followed by that executable: two bounded-record tests passed.
- `python3 -B -m unittest discover -s tools/tests -p 'test_*.py'`: 60 tests passed,
  including all three boot-profile tests and seven documentation-checker tests.
- Rebuilt all user programs/binaries/metadata using `tools/build-user.sh` through
  fresh native and desktop disk builds. All new probes were rebuilt before use.
- `RUSTOS_TEST_DISK=target/initator-desktop.img RUSTOS_CPUS=8 python3 -B e2e/desktop.py`:
  full desktop suite passed: separate WM/DE, exact wallpaper/geometry, launchers,
  commands/editor/arguments, concurrency, focus, drag/close, and DE-exit isolation.
- Documentation link checking and `git diff --check` passed.

### Native GRUB/Ring 3 acceptance

Build with `RUSTOS_BOOT_PROFILE=native-init ./tools/build-grub-disk.sh target/init-m1.img`.
Each command below runs its own disposable disk copy:

```bash
RUSTOS_TEST_DISK=target/init-m1.img RUSTOS_CPUS=8 python3 -B e2e/init.py --case two-services
```

| Case | CPUs | Observable acceptance |
| --- | --- | --- |
| `steady` | 1, 8, 32 | Ordinary native boot keeps both foreground services live through later heartbeat checks |
| `two-services` | 1, 8, 32 | First PID 1; distinct concurrent children; three failed runs; minimum 2/4-tick restart delays; cap reached; later Glow heartbeat |
| `fault-service` | 1, 8, 32 | Actual #UD in Flicker; typed Fault/6; bounded restart; Initator and Glow survive |
| `orphans` | 1, 8, 32 | Three failed parents; exited orphan status preserved; three live orphans forcibly stopped/reaped; surviving sibling |
| `abi` | 1, 8, 32 | Actual int 0x80 ownership/identity checks; reserved/high-bit rejection; denied display authority; invalid/read-only/overflow/guard output preserves status; valid cross-page output preserves guards; exactly-once reaping; bad executable rejection |
| `lifecycle` | 1, 8, 32 | 64 repeated spawn/terminate/reap cycles; exact frame baseline restored after remote quiescence |
| `exhaustion` | 1, 8, 32 | With 1 GiB RAM, 78 live children reached allocation failure; repeated failed construction added no frames; all children reaped and exact baseline restored |
| `fatal-exit` | 1, 8, 32 | Status 42, Exited; every CPU observed `HLT=1` through QEMU monitor registers |
| `fatal-fault` | 1, 8, 32 | Actual user #UD, Fault/6; every CPU halted |
| `fatal-thread` | 1, 8, 32 | Last-thread exit, Exited/0; every CPU halted |
| `missing-init`, `bad-init` | 8 | Explicit boot prerequisite panic before any process publication; all CPUs halted |

The complete **29-case** native acceptance matrix passed. Three additional
`steady` cases passed, for **32** acceptance cases total. After the final rolling
restart-window correction, the nine model-dependent supervision cases were rerun
against rebuilt Initator at 1/8/32 CPUs and passed; kernel/ABI/fatal probe code was
unchanged. The runner
retains serial traces, fatal register dumps and a summary under `target/init/`.
A PASS is emitted only after assertions; a timeout, startup log or silence is not
accepted as success. Disk images/logs/build caches are not committed.

Example captured from the 32-CPU supervision test:

```text
[initator] PID 1 entered Ring 3
[initator] started unit=0 run=1 pid=2 tick=1
[initator] started unit=1 run=2 pid=3 tick=1
[initator] completion pid=3 cause=Exited status=23 tick=3
[initator] started unit=1 run=3 pid=4 tick=5
[initator] completion pid=4 cause=Exited status=23 tick=7
[initator] started unit=1 run=4 pid=5 tick=11
[initator] completion pid=5 cause=Exited status=23 tick=13
[initator-test] restart-limit reached; glow remains running
```

The test additionally asserted a later Glow heartbeat. The 32-CPU fatal-fault dump
contained 32 halted CPUs, not merely a fatal banner. In the native recovery probes
the frame baseline was 19,595 pages after warm-up.

### Screenshot and manual limits

![Desktop compatibility from the M1-tested legacy build](screenshots/initator-m1-desktop.png)

This screenshot was captured and inspected from the fresh 8-CPU legacy desktop
regression build. It shows preserved 1920×1080 desktop presentation and launcher
contents, with service infrastructure excluded. It does **not** prove PID 1,
reaping or isolation; those are established by the native assertions above.
Additional test-generated screenshot changes were discarded in the isolated
worktree; existing user artifacts in the original worktree were not touched.

Manual host GTK/Wayland/X11 input/capture testing was not performed. QMP-driven
pixel/input assertions establish guest behavior only. This change adds no new
host display backend. The inherited bootloader `rustc-abi` target-field warning
remains visible. The documented unrelated debug heap-stress timeout and
nonterminating `gfx_demo` were not rerun or presented as passing.

## Limits and next milestone

This is trusted-service supervision. General user identities, filesystem/device
capabilities, resource scopes, public control/readiness IPC, configuration,
dependency graphs, persistent logs, watchdogs, graceful shutdown and graphical
sessions remain separate milestones. Legacy PID 0 display and permissive process
control stay confined to the default desktop profile during migration. Native
services still receive eager private screen-sized surfaces; lazy headless
presentation is deferred. Serial fatal reporting is best effort if its lock or
hardware is unavailable; the terminal path retains allocations and does not claim
DMA shutdown or durable poweroff. No live init recovery or GUI reconnection exists.

Next is M2: bounded unit files, validation, dependency/target transactions and
configured boot, using the same tested supervision engine. Keep one PR per
milestone and stack dependent branches without merging shared history.
