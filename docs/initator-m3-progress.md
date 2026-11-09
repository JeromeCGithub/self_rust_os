# Initator M3 implementation and validation

M1 [PR #54](https://github.com/JeromeCGithub/self_rust_os/pull/54) and M2
[PR #55](https://github.com/JeromeCGithub/self_rust_os/pull/55) are open and unmerged.
M2 documentation CI passed. Its validation is in
[the M2 record](initator-m2-progress.md). Branch `feat/initator-m3` starts from M2
commit `e56297c`; M3 is [PR #56](https://github.com/JeromeCGithub/self_rust_os/pull/56).
The original worktree is preserved.

**M3 is implemented, locally validated, and submitted as PR #56.** Channels, readiness,
watchdogs, volatile logging, Spark control, the Linux host backend and the native
launch worker are implemented. The original checkpoint evidence below is retained
as historical evidence; the [latest additions](#post-checkpoint-implementation)
record subsequent changes and final verification. M4–M7 remain future work.

## Implemented boundaries

- [ipc-abi](../crates/ipc-abi/src/lib.rs): explicit little-endian channel event,
  binding and v2 bound-spawn codecs. Canonical reserved fields, exact sizes,
  rights and limits are checked. Events carry a receiving handle and
  kernel-supplied ProcessKey; closure/writable signals cannot forge a sender.
- [ipc-core](../crates/ipc-core/src/lib.rs): the actual serialized kernel channel
  object model also runs in host tests. Limits: 128 global channels, 128 handles
  per owner, eight 256-byte messages per direction, eight explicit spawn bindings.
  Queues reserve fallibly at construction; enqueue allocates nothing. IDs never
  wrap or enter syscall scheduling/error sentinel ranges.
- [scheduler IPC](../src/scheduler/ipc.rs): syscalls 120–125 derive authority from
  the live current process, validate complete userspace buffers, copy before
  consuming and roll back unpublished pairs on copy failure. The existing
  scheduler lock serializes object operations with EVENT_PORT readiness/parking;
  no second object lock is introduced. Whole-process stop revokes endpoints
  before wakeups. Message delivery never uses a payload-supplied identity.
- [bound launch](../src/userspace/launch.rs): syscall 126 snapshots the 64-byte
  version-2 request, strings and bindings; revalidates grants after file loading;
  moves attenuated grants before child publication. Frames and parent handles
  survive failed launches correctly. Cwd's last fallible metadata allocation
  precedes frame/grant commitment. Old syscall 30's 48-byte v1 and connected
  stdio remain unchanged; v2 currently requires flags zero.
- [SDK IPC](../user_programs/userlib/src/ipc.rs): non-Clone owned endpoints close
  on Drop; bootstrap tags are consumed once. Successful spawn invalidates moved
  parent wrappers; failure preserves them. Send is nonblocking with explicit
  Full/Closed errors. Full interest masks support paused reads and one-shot
  writable events. Messages remain queued while reads pause; consumed EOF does
  not produce a permanent wake loop. Fair endpoint selection prevents one busy
  inbox from monopolizing delivery.
- [service ABI](../crates/service-abi/src/lib.rs) and
  [SDK client](../user_programs/userlib/src/service.rs): authenticated Init fixes
  the run context; Ready send failure remains retryable; Heartbeat is explicit
  work-path progress, not an automatic keepalive. Independent notify/log grants
  cannot be delegated by the child. UTF-8 logs reject controls and exceed neither
  the 232-byte text budget nor the 256-byte channel limit.
- [manager](../crates/service-core/src/manager.rs): Starting differs from Running.
  Startup deadlines begin at launch reservation, include pending publication,
  use checked arithmetic and reject Ready at the deadline. Watchdogs start at
  Ready; duplicate Ready cannot refresh them. Timeout requests one termination
  and waits for actual child completion. Exit zero before Ready still fails
  initialization; healthy reset begins at readiness. Restart policy remains
  bounded. Graph resource ownership is separate from readiness, so stopping an
  unready replacement or pending launch cannot acknowledge cleanup prematurely.
- [native ports](../user_programs/initator/src/ports.rs): independent per-run
  channels, sender/run/role validation, bounded 64-event batches and child-first
  completion handling. Notify/log budgets pause read interest and use deadline
  wakeups to resume, rather than polling. All fallible local budget setup happens
  before publishing a child. Exit retains attribution for one bounded final
  intake, then closes old grants before replacement. Volatile logging may lose
  tails during exit storms; child status never waits on logs.
- [journal](../crates/service-core/src/journal.rs): fixed-capacity records contain
  unit/run/process/tick/severity/text; no output I/O or allocation during intake.
  Native capacity is 256 records; overwrite counts and cursor gaps are explicit.
- [control request codec](../crates/service-abi/src/control.rs) and
  [authorization](../crates/service-core/src/control.rs): bounded allocation-free
  requests for status pages, log cursors/offsets, start/stop/restart/shutdown.
  Distinct kinds cannot decode as notifications. A connection is bound to an
  authenticated complete peer identity and run; read-only grants admit only
  status/log requests. Strictly increasing nonzero IDs reject replay without
  wraparound. Malformed or foreign requests cannot consume a legitimate ID;
  well-formed permission-denied requests do consume theirs. Authorization must
  precede lookup or effects. At this checkpoint these were not yet wired; see the subsequent implementation
  below. Shutdown remains unsupported until M4.

The full register/wire contracts are in [syscalls](syscalls.md#native-owned-channels-and-bound-spawn-m3-work-in-progress).
Configuration/readiness semantics are in [native units](initator-units.md).

## Red and green evidence

The evidence below concerns this unfinished M3 branch. M1/M2 historical results
are not substitutes for rerunning required regressions before the M3 PR.

Host tests, using the pinned nightly from `/tmp` outside bare-metal Cargo config:

```bash
cargo +nightly-2025-01-30 test --manifest-path /home/jay/personal/self_rust_os-initator/crates/ipc-abi/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /home/jay/personal/self_rust_os-initator/crates/ipc-core/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /home/jay/personal/self_rust_os-initator/crates/service-abi/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /home/jay/personal/self_rust_os-initator/crates/service-core/Cargo.toml --target x86_64-unknown-linux-gnu
```

- IPC ABI: six cases; objects: fifteen cases. Initially inert codecs/Registry
  failed their observable assertions. Later counter regressions caught handles
  equal to syscall sentinels and an overly restrictive private fairness counter.
  Tests cover FIFO, authenticated sender, attenuated atomic moves, stale/foreign/
  generation rejection, copy failure retention, stale acknowledgements, peer
  death, quotas, 512 cleanup cycles, fairness and writable rearming. Paused-read
  tests failed before read-interest support, then passed without dropping queues.
- Service protocol: six notify/client cases; two control-codec cases added next.
  Each set first failed against unavailable behavior, then passed. Control tests
  cover exact shapes, truncation/trailing bytes, canonical tails, invalid names,
  IDs, offsets, reserved words and separation from notification kinds.
- Core: 76 config/graph/transaction/supervision/readiness/journal cases, plus four
  new authorization cases. Readiness tests first failed against missing Starting/
  deadline behavior. Added regressions caught premature stop completion while an
  unready or unpublished replacement still owned resources. Journal tests failed
  before bounded storage/budget support. Four authorization tests first failed
  against an always-denied Session, then passed for read-only versus management,
  foreign/reused peer generations, stale runs, replay, malformed direct Rust
  requests, sequence exhaustion and connection replacement.
- `cargo test --test ipc`: initially failed pair creation, then bound spawn, then
  repeated bootstrap ownership. The implemented test passed with real kernel
  pointer/length/reserved/readonly/unmapped errors, retained copies, cross-process
  denial, blocking send wake, nondelegable grants, one-shot bootstrap, peer-exit
  wake and exact frame recovery. Its first live-thread rotation helper stalled
  because it incorrectly called `schedule`; the helper now uses the existing
  `on_timer` pattern. That stalled run was terminated and is not a green result.

Fresh build and regression checks also passed:

- `cargo build`, `cargo build --features grub`,
  `cargo build --release --features grub`, `cargo test --no-run` and
  `./tools/build-user.sh` (all applications). Compilation is separate from runtime
  results below. The dependency's existing unused `rustc-abi` target-field warning
  remains visible.
- `cargo test --lib`: 91 kernel unit cases.
- Seven integration executables: `ipc`, `display_mapping`, `keyboard_timer`,
  `native_supervision`, `process_supervision`, `stdio`, `threads`.
  The grouped regression run initially stopped at `stdio`: its old invalid-mask
  fixture used bit 64, now assigned to PORT. The test retains the same rejection
  expectation but derives the first unsupported bit from EVENT_ALL and also checks
  a mixed supported/unsupported mask. PORT success/block/wake is tested separately
  in `ipc`. `stdio` and the not-yet-executed `threads` then passed. This is an
  explicit ABI fixture migration, not a suppressed runtime failure.
- Documentation entry points/local links and all 62 host tooling tests passed.
- A fresh default-profile image (`target/init-m3-desktop.img`) passed the full
  `RUSTOS_CPUS=8 RUSTOS_TEST_DISK=target/init-m3-desktop.img python3 -B e2e/desktop.py`
  suite: separate WM/DE, wallpaper/geometry/edge input, shell/file/argument paths,
  concurrent clients, focus/drag/close and compositor survival after DE exit.
  [Captured desktop](screenshots/initator-m3-desktop.png) shows the tested legacy
  graphical profile, not Initator graphical-session supervision. Host GUI pointer
  integration remains a manual check; QMP injection cannot establish it.

Real Ring 3 validation uses private disk copies, fresh builder-generated
Initator/Flicker/Zinit binaries and the rebuilt native M3 kernel:

- `e2e/init.py --case ipc` and `--case ipc-lifecycle` passed at 1/8/32 CPUs.
  The first failed because the SDK and syscall 126 were missing. The working
  probe covers malformed raw calls, failed binding/file/ELF rollback, foreign
  handles, exact full-queue backpressure, one-shot writable, data-before-EOF,
  read pausing, bootstrap ownership and 64 exact-frame-recovery cycles per CPU
  profile. An early barrier wrongly used the PID-0 compositor mailbox; the test
  now uses a separate native control channel without widening GUI privileges.
- `e2e/init_units.py` cases `notify`, `notify-timeout`, `watchdog`, `heartbeat`,
  `wrong-ready`, `log-ready`, `foreign-ready`, `log-flood`, `log-exit` passed at
  1/8/32 CPUs (27 scenarios). Native notify initially failed without per-run
  grants. Dependents now wait for Ready after setup; missing Ready is killed and
  acknowledged before failure. A wedged work path has three capped watchdog
  runs. Valid completed-work heartbeats preserve one healthy run. Wrong run,
  readiness on the log endpoint and a known foreign manager handle cannot
  activate dependents.
- Log-pressure acceptance was added before its fixture mode: it failed when the
  fixture never announced readiness. The implemented fixture sends 512 bounded
  log attempts per work cycle, demonstrates both accepted and full results, and
  pulses its independent watchdog after completed work. A sibling wedges or exits
  with queued logs through three runs; PID 1 still reaps it and preserves healthy
  sibling progress. The first test completion barrier was too early (completion
  output precedes terminal state publication); it now waits for required Failed
  as well, without weakening assertions or increasing timeouts.

Retained native logs are under `target/init-units/{case}-{cpus}.log`; IPC logs are
under the runner's target evidence directory. Local red/green build/test transcripts
are `/tmp/initator-m3-*.log`. They are development evidence, not yet a curated PR
artifact. The confirmed 27-scenario transcript is
`/tmp/initator-m3-ready-matrix-confirmed.log`. Tests terminate/reap QEMU in finally.
Headless serial/process assertions demonstrate these features more directly than
a screenshot; no new graphical behavior or manual host GUI validation is claimed.

## Post-checkpoint implementation

The following code is present on the same M3 branch; no second M3 PR is planned.

- [Control replies](../crates/service-abi/src/replies.rs), the shared
  [dispatcher](../crates/service-core/src/control.rs), native
  [control connection](../user_programs/initator/src/control_port.rs), and
  [Spark](../user_programs/spark/src/main.rs) implement status, start, stop,
  restart, exact job queries and fragmented log queries. Control is an explicit,
  nondelegable per-run bootstrap grant (`ControlAccess=status` or `manage`).
  No public discovery endpoint exists. Read-only authorization precedes lookup.
  Increasing request IDs reject replay; malformed clients lose their control
  connection. One retained reply and bounded request intake prevent a slow client
  from blocking watchdogs. Shutdown is typed but returns Unsupported for management
  peers and Denied for read-only peers.
- Restart is one job covering old-child completion and replacement readiness.
  Existing root owners/dependents survive it. Restart budgets remain enforced,
  including visible terminal Failed state; explicit stop clears that state.
- [Linux development backend](initator-host.md) runs real foreground children,
  bounded streams and the same core. A pidfd identifies each Linux child for poll
  and termination. A single bounded spawn worker leaves the actor free to service
  deadlines. Queue saturation applies backpressure rather than misclassifying an
  admitted service as failed. The seven runtime tests and two CLI tests passed
  again after the cancellation changes.
- [Native worker](../user_programs/initator/src/launcher.rs) performs filesystem
  loading in 16 KiB reads with userspace yields between them. Syscall 127 snapshots
  a complete immutable executable buffer and uses the existing process constructor
  with atomic explicit grants. This kernel addition was necessary: kernel-mode PIO
  does not preempt on one CPU, so merely moving synchronous path spawn to another
  userspace thread still delayed a six-tick watchdog by eleven ticks under an
  explicit 8 MiB/s disk throttle. With chunked loading, the prior 1/8/32 CPU
  experiment met the unchanged two-tick/500 ms tolerance. This is a measured bound
  for the fixture, not a hard real-time guarantee or arbitrary-size mapping bound.
- Only the actor owns manager state and process-wide event consumption. A fixed
  early-completion buffer handles exits consumed before worker publication.
  Results are durable before private wake delivery; the actor rechecks the result
  predicate immediately before parking. Worker Drop cancels, joins, reaps any
  unpublished child and closes both private endpoints.
- Cooperative cancellation uses an exact unpublished launch token. It stops pending
  chunked I/O when ownership is withdrawn or startup expires. Cancellation cannot
  undo a process already committed by the kernel: such results remain supervised
  and are killed/reaped before job completion. Stale tokens cannot cancel a later
  launch. The core regression first failed against a false query, then passed.
  A real Ring 3 probe first failed against missing outstanding identity, then
  passed on one CPU after worker support was added.
- Async publication required an explicit migration of the early-exit evidence:
  admission must precede the observed prerequisite exit; publication can follow it
  only with exact stop/reap before rollback failure and no intervening Running
  state. Five new verifier tests cover valid synchronous/async/cancelled traces
  and reject late admission, stale tokens, wrong child, missing stop, premature
  failure and duplicate publication. They first failed against an inert verifier.

Current rerun transcripts are retained in `target/initator-m3-evidence/`, including
`core-cancel-full.log` (95 core tests), `host-cancel-full.log` (nine host tests),
`cancel-token-red.log`, `cancel-token-green.log`, `cancel-worker-red.log`,
`cancel-worker-green.log`, and the rollback/latency verifier red/green logs.
Earlier `/tmp` transcripts mentioned above belonged to the checkpoint session and
are no longer retained. Do not treat their paths as newly captured evidence.

The complete pre-cancellation unit rerun passed all 28 cases at 1 and 8 CPUs,
then failed the 32-CPU early-exit assertion because it compared publication rather
than admission. That failed run is retained as `units-final-matrix.log`; it is not
an overall pass. The later M1 ABI run exposed another obsolete invalid-mask fixture:
bit 64 is now PORT, so an intended rejection call blocked. Like the earlier stdio
fixture migration, the probe now derives `EVENT_ALL + 1` and checks a mixed supported/
unsupported mask without changing the expected error. Its timeout remains visible
in `m1-final-matrix.log`; the corrected real Ring 3 probe passed before the full
base-supervision rerun.

## Final verification on 2026-11-09

[Retained test output](initator-m3-evidence.txt) contains the red assertions and
final command summaries. These are fresh reruns, distinct from the checkpoint
results above:

- 95 pure core tests; seven IPC ABI, fifteen channel-object and eleven service ABI
  tests; nine real Linux runtime/CLI tests. Total: 137 host Rust tests.
- 75 Python tooling tests and documentation entry-point/local-link checks.
- All applications rebuilt with `tools/build-user.sh`; default and GRUB debug,
  GRUB/native-init release, and `cargo test --no-run` builds succeeded. Compilation
  is separate from the following execution results.
- `cargo test --lib`: 91 kernel runtime cases. The seven affected integration
  executables (`ipc`, `display_mapping`, `keyboard_timer`, `native_supervision`,
  `process_supervision`, `stdio`, `threads`) all passed.
- All 28 configured-unit scenarios at 1/8/32 CPUs: **84 passes**, including complete
  startup validation, Ready failures, watchdog/log pressure, readonly/management
  control, reply backpressure and referenced-service restart.
- All twelve original supervision scenarios at 1/8/32 CPUs: **36 passes**, including
  fatal PID 1/all-CPU halt, ABI denial, lifecycle/frame recovery, adoption,
  allocation exhaustion, missing/bad init and two live services with bounded restarts.
- Native IPC and 64-cycle IPC lifecycle at 1/8/32 CPUs: **six passes**, including
  image startup, pre-publication child exit, consumed wake, exact cancellation
  tokens, joined/discarded workers and frame recovery.
- Independent and required-dependent loader experiments at 1/8/32 CPUs with an
  explicit 8 MiB/s private-disk read throttle: **six passes**. All had zero extra
  watchdog ticks and 324–336 ms observed Ready-to-stop time against the unchanged
  two-tick/500 ms tolerance. Independent large loads completed in 47/49/49 ticks;
  cancelled dependent loads acknowledged after six ticks without publication.
  Required-dependent rollback collected all four existing children before Failed.
  QEMU throttling uses its documented
  [drive option](https://www.qemu.org/docs/master/system/invocation.html).
- A freshly rebuilt default profile passed the complete eight-CPU desktop suite.
  [Screenshot](screenshots/initator-m3-desktop.png) shows its actual wallpaper,
  compositor and Portal terminal during intentional shell-error fixtures. This is
  legacy desktop compatibility evidence, not native graphical-session supervision.
  Manual host GUI pointer integration remains untested.

Commands for native reproduction (builds write only development images; runners
copy them to private writable fixtures):

```bash
RUSTOS_BOOT_PROFILE=native-init ./tools/build-grub-disk.sh target/init-m3-final.img
RUSTOS_TEST_DISK=target/init-m3-final.img RUSTOS_CPUS=32 python3 e2e/init_units.py --case control-manage
RUSTOS_TEST_DISK=target/init-m3-final.img RUSTOS_CPUS=32 python3 e2e/init.py --case ipc
RUSTOS_TEST_DISK=target/init-m3-final.img RUSTOS_CPUS=1 RUSTOS_INIT_READ_BPS=8388608 python3 e2e/init_latency.py
RUSTOS_TEST_DISK=target/init-m3-final.img RUSTOS_CPUS=1 RUSTOS_INIT_READ_BPS=8388608 RUSTOS_INIT_CANCEL_DEPENDENT=1 python3 e2e/init_latency.py
./tools/build-grub-disk.sh target/init-m3-desktop-final.img
RUSTOS_TEST_DISK=target/init-m3-desktop-final.img RUSTOS_CPUS=8 python3 -B e2e/desktop.py
```

The latency fixture requires builder-generated `zload` (`./tools/build-user.sh zload`),
which is test-only and not installed in ordinary profiles. Full scenario lists are
in the runner's `--help`. Headless supervision assertions and exact serial event
traces demonstrate these changes more directly than a screenshot of the console.
No current required check remains failing. The documented existing target warning
and manual host GUI gap remain. The single M3 PR targets `feat/initator-m2`; it remains open and unmerged.
Documentation CI is checked separately from the local runtime evidence.

The global channel quota does not reserve capacity against hostile allocators.
Processes remain trusted until M5 resource/authority enforcement; endpoint ownership
and attenuation are enforced already. Native identities still use generation one
and monotonic unreused PIDs. No multiuser/session/display isolation or persistent
journal is claimed. Shutdown/scopes, credentials/resources, Nest/compositor lifecycle
and final hardening remain M4–M7.
