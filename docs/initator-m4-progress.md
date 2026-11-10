# Initator M4 work in progress

M3 is submitted as [PR #56](https://github.com/JeromeCGithub/self_rust_os/pull/56),
stacked on M2. Its documentation CI passed at `8a4a432`. The full M3 runtime and
compatibility evidence is in [the M3 record](initator-m3-progress.md).

M4 is on `feat/initator-m4`, based on that commit. **M4 is implemented, locally validated and submitted as [PR #57](https://github.com/JeromeCGithub/self_rust_os/pull/57).**
The additions span userspace stop/shutdown policy and a minimal native kernel halt
operation with CPU/device checks.
M5–M7 remain future work; no user/session/resource isolation is claimed.

## Implemented stop behavior

- Service protocol kind 5 is a canonical 24-byte `Stop { run }` message, sent on the
  existing manager-to-service notify direction. The initialized manager's exact
  authenticated identity and run must match. Repetition is idempotent. The SDK's
  `Client::stop_requested()` lets the application's ordinary event loop begin
  cleanup and exit; no signal handler, callback thread or executor is introduced.
  A stopped client cannot announce Ready or heartbeat, but may log its cleanup.
- `StopTimeoutTicks` is a positive service-only singleton, default 90, with the
  same checked signed-tick range as other deadlines. Targets reject it even when
  Kind appears last. The previous unknown-key fixture now uses the still-unsupported
  `StopTimeoutMilliseconds`; dedicated tests establish the new supported field.
- The core emits `Action::NotifyStop` once for ordinary stops, including restart
  and rollback. Startup/watchdog failures still force termination immediately.
  Expired grace or failed delivery emits one termination and removes the expired
  deadline from wait interest. Completion of a stop job always requires an actual
  child exit; sending Stop or issuing kill does not acknowledge cleanup.
- Delivery-failure callbacks require exact unit/run/process identity and an
  outstanding stop. They cannot shorten a newer run's grace. Deadline overflow
  forces cleanup without wrapping a timestamp.
- Native ports retain a bounded pending Stop, send Init first, handle writable
  readiness without polling and report a closed delivery path to the manager.
  The actor distinguishes a first stop from escalation in serial diagnostics.
  The host backend likewise preserves partially written Init before its separate
  bounded Stop frame, handles write errors and sleeps on existing stream/pidfd
  readiness.

Existing core lifecycle tests explicitly migrate ordinary-stop effects from
Terminate to NotifyStop while retaining their actual-completion, stale-result,
readiness and restart-budget assertions. Startup/watchdog kill assertions remain.
New tests separately require successful cooperative exit, grace expiration, exact
failure identity, overflow and no duplicate escalation.

## Implemented shutdown core

`Manager::shutdown(timeout)` creates one terminal job, replacing activation and
withdrawing all root ownership. Repeated requests coalesce without extending its
deadline. Start/stop/restart requests are thereafter Busy. All restart desire and
queued-launch admission are withdrawn immediately, while live services retain their
state until ordered stop. Reservations remain owned until cancellation failure or
raced publication/completion acknowledges them.

The first actor step sets a checked global deadline. Reverse resource order stops
dependents before prerequisites; a global expiry forces every remaining owned
process without waiting for serial grace periods. No new Ready/heartbeat can rearm
health after shutdown admission. `shutdown_ready()` requires completed cleanup and
no owned spawning/starting/running/stopping lifetime. This is a foreground core
predicate, **not permission to halt the machine**: the native kernel must still
verify remaining processes and device/CPU quiescence. Descendant scopes remain M5.

## Red/green evidence captured on 2026-11-09

Local transcripts are in `target/initator-m4-evidence/`:

- `service-abi-stop-red.log`: two tests failed at Unsupported before Stop encoding
  and client state existed. The implemented service protocol's thirteen tests pass.
- `service-core-stop-red.log`: configuration failed at UnknownKey. The dedicated
  field tests pass after parser support.
- `stopping-red.log`: four behavioral failures showed immediate kill, absent grace
  and missing overflow classification. All four now pass.
- `core-stop-regression.log` retains expected old-effect assertion failures before
  explicit migration; `core-stop-green.log` passed afterward. The later
  `core-initial-full.log` passes all 105 core tests, including five shutdown cases.
- `stop-client-red.log`: a real Ring 3 child exited status 1 because the SDK treated
  Stop as a second Init. `stop-client-green.log` confirms clean authorized Stop and
  exit status 0 at 1/8/32 CPUs using private image copies.
- `host-stop-red.log`: actual children were killed instead of cleanly exiting, or
  killed before grace. The eleven host runtime/CLI tests then passed in
  `host-stop-green.log` and `host-initial-full.log`.
- `native-stop-red.log`: repeated managed services never executed cooperative
  cleanup. `native-stop-green.log` passes `control-cooperative` at 1/8/32 CPUs:
  seventeen distinct runs per profile announce cleanup, exit zero without a kill,
  complete their stop jobs and recover exact frame counts through Spark's probe.
- `shutdown-red.log`: five core cases failed with Busy before terminal-job support;
  `shutdown-green.log` passes reverse order/actual acknowledgement, global deadline,
  pending launch/raced publication, restart-backoff cancellation and overflow.
- `host-shutdown.log`: two additional real-process integration tests pass ordered
  cooperative shutdown and forced cleanup of all hung children at the global
  deadline. These were additional integration checks of the tested core, not a
  claimed separate red/green implementation cycle.

## Native terminal action and completed integration

[Syscall 128](syscalls.md#terminal-system-action-m4) accepts Halt only, with zero
reserved words. Kernel-selected native-init authority is checked before parsing;
other process records, including zombies, return Busy after valid scalar parsing.
Reboot/poweroff remain Unsupported. Refusal never mutates devices or parks CPUs.

Commitment parks other CPUs through the existing NMI terminal path, disables
initialized E1000 packet engines/interrupts and PCI bus mastering, and performs
bounded AC97 stop/reset before disabling bus mastering. No device/frame allocation
is freed; no user return or entry release occurs after commitment, even if an
acknowledgement/readback fails. Diagnostics report the actual parked/DMA flags.
This is Halt, not poweroff, reboot, storage durability or generic power management.
The E1000 register sequence follows the
[Intel 8254x manual](https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf).
AC97 uses the driver's existing bounded stop/reset mechanism; no new mixer or
hardware policy is introduced.

`Spark shutdown` requires management authority. It admits one terminal job with a
180-tick global budget and prints its admitted job. PID 1 drains that job, keeps
itself alive and invokes the typed `Result<Infallible, Error>` halt wrapper. Busy
leaves PID 1 collecting durable adopted-child events. Its idle launch worker is
part of PID 1; core readiness implies no unacknowledged launch remains, and terminal
halt retains the complete process and stops its CPUs.

Additional red/green regressions:

- `system-action-abi-red.log`: Halt/scalar validation and new typed errors failed
  before ABI support; all three platform tests now pass.
- `system-action-kernel-red.log`: no-authority calls returned Invalid rather than
  Denied. The implemented gate passed the kernel suite afterward.
- `halt-abi-red.log`: real Ring 3 callers reached an unknown syscall. Native
  preflight checks then passed at 1/8/32 CPUs for ordinary-child denial (including
  malformed requests), reserved words, unsupported actions and retained-child Busy.
- `terminal-red.log`: the preflight-only Halt returned Unavailable. After terminal
  commitment/device stop, all nine halt cases passed: network present, actually
  queued/running AC97 PCM, and no supported devices, each at 1/8/32 CPUs. The runner
  checks QMP HLT state for every CPU and independently reads PCI command registers
  through monitor port I/O, requiring bus mastering clear. Serial text alone cannot
  satisfy these assertions; fatal init is rejected.
- `shutdown-control-red.log`: management Shutdown was still Unsupported. The
  dispatcher now admits/coalesces the terminal job; read-only denial remains tested
  before any mutation. `native-shutdown-red.log` then caught the absent Spark
  command before the executable/actor path was implemented.
- `shutdown-restarted-order-red.log`: three tests found that reverse creation order
  could stop a restarted prerequisite before its dependent, including through a
  processless target and separately activated After ordering. Cleanup now computes
  a bounded selected ordering closure and stops dependents first; reverse activation
  is only an independent-unit tie-breaker. The M2 transaction expectation explicitly
  migrates to this M4 requirement, retaining its ownership/liveness assertions.
- `unready-stop-order-red.log` caught premature orphan cleanup of an unready
  replacement while its dependent still stopped. Resource ownership now preserves
  that ordering, while restart desire and pending I/O are withdrawn immediately.
  A companion test confirms withdrawn heartbeats cannot rearm watchdogs. The full
  core suite passed after the fix.

## Final verification captured on 2026-11-09

These checks were rerun against the final implementation, independently of the
M3 evidence. [Retained output](initator-m4-evidence.txt) records command results;
full local logs remain under `target/initator-m4-evidence/`.

- **170 host Rust tests:** core 111, service ABI 13, platform ABI 3, IPC ABI 7,
  IPC objects 15, Linux runtime/CLI 13, network stack 6 and network ABI 2.
- All **75 Python tooling tests**, documentation/local-link checks and diff checks.
- All userspace binaries/metadata regenerated together; default and GRUB debug,
  GRUB/native-init release and test compilation passed. Existing target-field
  warnings remain visible; compilation is separate from runtime verification.
- **92 kernel runtime unit cases** and all seven affected integration executables:
  IPC, native/process supervision, stdio, threads, keyboard/timer and display mapping.
- Native boot/ABI/channel/halt runner: **57 scenarios** at 1/8/32 CPUs.
- Configured service/control runner: **90 scenarios** at 1/8/32 CPUs, including
  seventeen cooperative or forced runs per profile, exact frame recovery, closed
  Stop delivery and unchanged watchdog/readiness/authorization pressure cases.
- Shutdown runner: **15 scenarios** at 1/8/32 CPUs. Cooperative shutdown requires
  exact exit-zero completions and dependent-before-prerequisite ordering. Hung
  services use 1000-tick local grace but are killed at the 180-tick global deadline.
  Final review strengthened this case to require drained completion no later than
  three ticks after the global deadline, plus distinct process identities. The
  strengthened 1/8/32 runs passed in `global-bound-confirmed.log`.
  Other cases cancel an active slow loader without publication, cancel restart
  backoff, and preserve stop ordering after prerequisite restart. Every started
  process completes before drained state, and every CPU is actually halted.
- Six isolated 8 MiB/s loader/cancellation experiments retained the unchanged
  two-tick/500 ms tolerance: all had zero extra watchdog ticks and 323–336 ms
  observed Ready-to-stop time. Independent loads completed in 47 ticks; cancelled
  loads acknowledged after six ticks. Warmup rollback now also exercises M4 grace.
  These scenario measurements are not a hard real-time guarantee under arbitrary
  kernel calls, hardware or host load.
- Fresh eight-CPU default desktop suite passed. Its
  [screenshot](screenshots/initator-m4-desktop.png) shows actual legacy compositor,
  wallpaper and terminal error fixtures, not Nest/session supervision. Manual host
  GUI integration remains untested.
- Eight-CPU network compatibility: real Ring 3 UDP/TCP/HTTP ownership/buffer probes,
  absent NIC, and 48 packet-verified gateway ping replies across ring reuse passed.
  No public Internet acceptance is inferred from gateway replies.
- Eight-CPU audio compatibility: independently captured non-silent stereo PCM and
  absent-hardware typed NoDevice passed. QEMU emits input-voice warnings for its
  output-only WAV backend; the captured output and active DMA assertions pass.

Reproduction uses freshly built disposable source images; every runner boots its
own writable copy:

```bash
RUSTOS_BOOT_PROFILE=native-init ./tools/build-grub-disk.sh target/init-m4-final.img
RUSTOS_TEST_DISK=target/init-m4-final.img RUSTOS_CPUS=32 python3 e2e/init.py --case halt-audio
RUSTOS_TEST_DISK=target/init-m4-final.img RUSTOS_CPUS=32 python3 e2e/init_units.py --case control-cooperative
RUSTOS_TEST_DISK=target/init-m4-final.img RUSTOS_CPUS=32 python3 e2e/init_shutdown.py --case restarted-prerequisite
RUSTOS_TEST_DISK=target/init-m4-final.img RUSTOS_CPUS=1 RUSTOS_INIT_READ_BPS=8388608 python3 e2e/init_latency.py
```

The final native matrix log is `final-native-matrix.log`; latency uses
`latency-final-matrix.log`. The full 1/8/32 matrix passed before evidence curation.
Earlier failed runs remain visible; no timeout or expected runtime result was
weakened. Lifecycle assertions migrate only where M4 intentionally adds cooperative
stop or dependency ordering.

## Limits and next milestone

No current required local check is failing. The single M4 PR targets
`feat/initator-m3` and remains open and unmerged. Documentation CI is checked
separately from runtime evidence.

Halt covers initialized supported DMA drivers, not arbitrary firmware devices or
power management. A post-commit hardware/CPU acknowledgement failure still enters
the terminal loop with failure flags and all memory retained; it cannot safely
return to userspace. No storage durability or physical-hardware validation is
claimed. Volatile logs may lose bounded exit tails. Descendant adoption is handled,
but enforced descendant scopes, user/session identities and resource isolation are
M5; endpoint factory quotas still do not reserve hostile-user capacity. Nest and
compositor/session lifecycle are M5–M6; M7 supplies the remaining hardening.
