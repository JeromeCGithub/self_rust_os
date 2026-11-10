# Initator M5 work in progress

M4 is [PR #57](https://github.com/JeromeCGithub/self_rust_os/pull/57), open and
unmerged, with passing documentation CI at `7f8b750`. Its local runtime evidence
is in [the M4 record](initator-m4-progress.md). M5 starts from that commit on
`feat/initator-m5`; no M5 PR exists yet.

**M5 is incomplete.** The first addition is a host-tested principal description and
attenuation policy in [process-abi](../crates/process-abi/src/principal.rs). The
kernel now attaches native principals, enforces mechanism capability domains and
supports one-way process rights reduction and protects system-file mutations.
Kernel-only scoped construction, page/thread accounting and descendant cleanup
are now wired. The Ring 3 scope grant/event ABI, session home/tmp grants and
filesystem grants/remaining allocation paths and socket/audio accounting remain
incomplete; Nest does not yet run.
IPC endpoint/inbox budgets, atomic grant accounting, descriptor/buffer leases and
owned executable/startup snapshots are now wired below.
Syscalls 129–130 and their SDK wrappers are wired. The remaining authority/resource
enforcement must be implemented in this same milestone before delivery. Do not interpret a decoded description as a
grant.

## Initial interface

The version-1 descriptive principal record is exactly 40 little-endian bytes:
version u32 at 0, flags u32 at 4, native user u32 at 8, zero reserved u32 at 12,
session u64 at 16, scope u64 at 24 and known capability bits u64 at 32. The system
scope reserves ID 1. Non-system users require a nonzero session and a non-system
scope. Init and legacy flags cannot be combined, nor attached to a user/session
context. Truncation, extra bytes, unknown rights/flags and reserved identities fail.
These are native identities, with no Linux ABI or account database implied.

Capability domains describe file reads/writes, protected-system writes, IPC
factories, process/thread creation, networking, audio and scope creation. They
remain upper bounds: future kernel scope and object-owner checks must enforce the
actual authority. There is no general administrative flag supplied by applications.
Ordinary child construction preserves user/session/scope, drops init, protected
system writes and scope-factory authority; explicit attenuation only removes bits.
Existing irreversible worker restrictions must remain a stricter intersection.

## Source findings guiding the next changes

- [Process metadata](../src/process/mod.rs) originally lacked principal fields;
  M5 now stores native identity and mechanism rights there. It also contains parent/adoption
  ownership, private frame/surface ledgers,
  descriptors and an optional restricted-worker heap ceiling. Threads share their
  process resources. The process table has 128 reusable storage slots but externally
  monotonic, unreused PIDs; scope membership must not use a PID as a bit index.
- [Common construction](../src/scheduler.rs) is the single Process initializer;
  all five existing spawn paths now inherit an ordinary-child principal there
  before grant/frame publication. Explicit scoped grants now exist as a kernel-only interface. Current construction initializes the
  worker ceiling to None; the existing worker syscall allowlist prohibits spawn.
- [Worker permission filtering](../crates/process-abi/src/isolation.rs) is a bounded
  allowlist, not general user identity or delegation. The dispatcher applies it
  before copying or allocating for a syscall. Future user authority checks belong
  at that boundary and at the underlying object/side-effect boundaries.
- [File handles](../src/fs/handles.rs) cap individual I/O at 64 KiB but permit up to
  1024 process descriptors. Path resolution preserves FAT component spelling and
  handles cwd/`..`; it is not an authorization check or stable-entry grant. M5 must
  protect resolved ancestors/entries under the FS lock, including aliases and
  create/truncate effects, across descriptor and old path syscalls.
- [Frame transactions](../src/memory/owned.rs) own all unpublished allocations and
  roll back on failure. [Heap growth](../src/scheduler/heap.rs) presently enforces
  the restricted worker's heap ceiling and native scope limits; initial image,
  stack, surface, page tables and threads also retain native resource charges. Charges and physical
  reclamation have distinct lifetimes.
- The default desktop is still the trusted legacy profile. Native M5 session
  enforcement must not be described as protection already supplied to that path.
  Non-PID-0 graphical authority/discovery and compositor lifecycle remain M6.

## Tests run so far

`target/initator-m5-evidence/principal-red.log` records three expected failures:
missing codec, inherited init authority and unavailable valid attenuation.
`principal-green.log` records those three new tests and the existing process-ABI
suite passing after implementation. The tests cover exact wire shape and reserved
bytes, short/trailing data, inconsistent system/user/legacy contexts, invalid IDs,
ordinary-child identity/scope preservation, non-inheritable authority and attempted
rights expansion. These are host policy/codec results, not native enforcement.

## Kernel query and ordinary inheritance

[The principal syscall](../src/scheduler/principal.rs) validates exact 40-byte output,
zero reserved argument and every writable page, snapshots a live caller under the
scheduler lock, then copies initialized canonical bytes without retaining a process
or user reference. It has no PID selector and accepts no input identity. Kernel
boot gives native PID 1 the Init flag; other kernel-origin system contexts and the
legacy desktop profile are explicitly distinguished. Every ordinary spawn derives
its native identity and initial mechanism rights from the live parent in the common constructor. Threads
read the same process description. Restricted workers keep their earlier allowlist;
the new query does not bypass it.

[The SDK](../user_programs/userlib/src/principal.rs) queries into an initialized
40-byte buffer and validates the returned codec. `principal-native-red.log` first
failed because PID 1 had no available query. After wiring, `principal-native-green.log`
passes on 1/8/32 CPUs with all five existing spawn interfaces (8/29/30/126/127),
children/grandchildren, per-process thread identity, environment spoofing, readonly/
unmapped/noncanonical/wrapping buffers, exact sizes/reserved words, crossing two
writable pages, preserved guard bytes and overwriting fake input metadata.

Normal boot still launches system contexts: user 0/session 0/reserved system
scope 1. The kernel-only scope adapter below can now construct distinct identities,
but the Ring 3 scope/session factory, per-session filesystem grants and other
resource domains remain incomplete. A capability description is not proof that
all remaining resource/path boundaries exist.

Initial integration checks also pass: 92 kernel unit cases, the four directly
affected integration executables (`native_supervision`, `process_supervision`,
`ipc`, `threads`), both default/GRUB builds, a fresh native release image and all
rebuilt user programs. The fresh default-profile eight-CPU desktop suite passed
with the new metadata/query SDK. This is compatibility evidence, not enforcement
of user isolation. All transcripts are `principal-*.log` in the M5 evidence folder;
these are intermediate checks, not the final M5 milestone matrix.

## Mechanism enforcement and one-way reduction

[Capability policy](../crates/process-abi/tests/capabilities.rs) first failed three
host tests with allowed ambient operations, unsafe open modes and unauthorized
audio resume. The implemented explicit allowlist passes the complete process-ABI
suite. It distinguishes standard I/O from regular descriptors, includes O_CREAT
side effects with read-only access, rejects malformed open flags and denies future
unknown syscall numbers. Owned IPC and cleanup remain distinct from factory rights.
The existing worker allowlist remains a stricter intersection.

[The native probe](../user_programs/zinit/src/capabilities_probe.rs) first failed
at the missing rights-reduction syscall (130). After implementation it passes on
1/8/32 CPUs: native init refusal, unknown bits/reserved words and expansion leave
authority unchanged; sibling threads prevent reduction; single-threaded reduction
is irreversible/idempotent; ordinary children inherit reduced rights; regular file
handles lose read/write authority; all old/new spawn paths, threads, factories and
device operations are denied; file create/overwrite/mkdir have no denied side
effects; owned channel delivery/interest/close and file close/wait/child cleanup
still work. A connected restricted worker cannot call 129 or 130. A caller with
only FILE_READ can read but cannot use read-only create; FILE_WRITE permits a
write while still denying reads/read-write opens. The native system-action gate
retains its typed Denied response independently of mechanism rights.

Reduction requires exactly one owned thread, including unreaped records, because
filesystem calls can release the entry gate during I/O. The entry gate and scheduler
lock make validation/publication atomic; no user pointer is involved. This call
revokes neither pre-existing children nor granted objects. It does not replace
scope-wide containment, accounting, protected roots or explicit launch grants.

`target/initator-m5-evidence/capabilities-host-{red,green}.log` and
`capabilities-native-{red,green}.log` retain red/green output. Intermediate checks
also pass: 92 kernel unit cases; five integration executables (`native_supervision`,
`process_supervision`, `ipc`, `threads`, `stdio`); default/GRUB builds, native GRUB
release, all rebuilt applications and test compilation. Eighteen native regressions
cover principal queries, two-service supervision, child ABI, channel validation,
cooperative stop and halt authorization on each of 1/8/32 CPUs. The eight-CPU
Spark cooperative-stop and whole-system cooperative-shutdown cases also pass.
The fresh default-profile eight-CPU desktop suite and 75 Python tooling tests
also pass. These are checks for this increment, not the final M5 acceptance matrix.

## Protected system-file mutations

Native ordinary writers cannot modify `/BIN`, `/ETC` or `/kernel.elf` without the
non-inheritable SYSTEM_WRITE right. PID 1 retains recovery/configuration writes.
[Filesystem authorization](../src/fs.rs) runs under the same filesystem lock as
create/truncate/write, with [FAT entry resolution](../src/fat32.rs) comparing the
actual root entry for long/short aliases. Missing root labels remain reserved.
Canonical path handling supplies cwd, case and `.`/`..` semantics before this
check. Both old path writes/mkdir and descriptor open/write are guarded. Metadata
is snapshotted before taking FS locks, preserving scheduler/FS/open-file lock order.
The legacy profile is explicitly trusted, including inherited legacy flags; this
is not a claim of new isolation for the default desktop.

The new kernel test first failed on an unprotected reserved name. The native probe
first reproduced an ordinary child overwriting `/ETC/SYSLOCK.TXT`. After wiring,
93 kernel unit cases and 1/8/32-CPU native system-file cases pass: old path writes,
writable/open/create/truncate modes, relative/cwd/parent/case/alias paths, protected
executables and boot kernel, no failed-create/mkdir side effects, intact fixture
bytes, and 32 denied opens without leaking a descriptor reservation. Unprotected
sibling prefixes remain writable; PID 1 can truncate/write the protected fixture.
Known-right expansion cannot mint SYSTEM_WRITE. The RAM-backed FAT test covers
protected long-name root directories/files through their 8.3 aliases, aliases as
labels, malformed/non-normalized paths and missing reserved entries.

Evidence is in `target/initator-m5-evidence/system-files-*.log`: red kernel/native
failures, green kernel/native results, fresh images and all rebuilt applications.
The twelve 1/8/32-CPU cases also recheck reduction, query/inheritance and two-service
supervision. Default/GRUB builds, native GRUB release, test compilation and the
FAT32 fixture integration pass. The existing real Ring 3 file-descriptor suite
also passes on 1/8/32 CPUs, and the fresh default-profile eight-CPU desktop suite
passes, preserving its explicit legacy exception. Generic desktop screenshots
from this exact build are retained under the M5 evidence directory. Per-session
read/write roots, object delegation, disk quotas and scope quiescence are still
missing. No untrusted user session is launched by this increment.

## Scope ledger and host model

[scope-core](../crates/scope-core/src/lib.rs) follows the existing IPC-core split:
an allocation-free `no_std` object registry shared with host tests, with all
kernel context, locking, syscall usercopy and actual destruction left to adapters.
Its initial standalone increment exposed no syscall; the kernel adapter below now
links it into the scheduler. Its 64 fixed records
include system root 1, session roots and service leaves. Fresh nonwrapping IDs
never alias a released slot. Four slots and a configurable vector of resource
reserves remain available to system recovery. The system root cannot be closed.

Only the kernel-selected init key creates sessions. User 0 and reserved identities
are refused for session creation; same-user sessions still get distinct IDs.
A once-appointed leader can create service scopes under its own session, but
cannot close the session grant owned by init, create arbitrary recursive scopes,
or become the factory for a second independent session. Native construction must
bind the actual newly created process and check capabilities; no caller-provided
key or decoded principal may substitute for kernel context.

Linear opaque charges account processes, threads, private physical pages, selected
kernel heap bytes, descriptors, IPC handles, sockets and audio streams. Every delta
checks all components and ancestors before any change. Failure/overflow preserves
all counts. Closing prevents new reservations/growth. Release is permitted during
closure and must occur only after real resource release, including frame/TLB,
thread-stack and filesystem-call quiescence. The ledger does not perform those
operations or assert that the currently unwired counters already contain them.

Leader exit closes its scope and descendants; dead controller ownership passes
to init. Scope closure waits for all charges and child records. Notifications are
peeked durably, then acknowledged explicitly, without consuming a process status.
Children are acknowledged before the parent becomes closed. The native adapter
must expose readiness for these events, kill closing members, and keep service
stop/restart jobs pending until scope cleanup is acknowledged.

`scopes-host-red.log` records seven missing-feature failures. The first seven
scenarios then passed. `scopes-factory-red.log` caught duplicate session factory
appointment; after rejecting duplicate leader keys, `scopes-factory-green.log`
passes 11 runtime tests and two compile-fail tests for opaque/non-cloneable charges.
Coverage includes ancestor pressure, all eight resource bounds/overflow, atomic
rollback, init recovery reserves, cross-session denial, pending cleanup owner death,
leader exit, stale identities, 128 create/close/ack cycles, sequence exhaustion,
bootstrap failure and durable child-before-parent closure.

The standalone bare-metal check (`scopes-native-check.log`) is compilation evidence
only. The later kernel integration test below adds actual mapped-resource checks;
full Ring 3 scope/session acceptance still requires the remaining resource and
syscall adapters. Keep host model results separate from native execution results.

## Reservation guards and frame-budget prerequisite

The scope model now reports per-resource remaining allocator room across all
ancestors, including the system recovery reserve. `scopes-room-red.log` first
failed on the missing room calculation; its green suite covers parent pressure,
recovery consumption and closed/stale scopes. A later mixed-resource regression
(`scopes-mixed-pressure-red.log`) caught thread recovery pressure incorrectly
blocking page-only allocation. The fix checks only resource components that grow,
while preserving all overflow, ancestor and atomicity checks. The final
`scopes-mixed-pressure-green.log` passes 16 runtime cases and both compile-fail
checks.

[Reservation/growth guards](../crates/scope-core/src/transaction.rs) hold an
exclusive registry borrow across fallible construction. Dropping a new reservation
returns its counts and restores an unpublished leader binding; committing transfers
the unique charge to its resource owner. Growth rollback releases only its new
delta, preserving the owner's earlier charge. The native adapter must destroy
actual allocations before dropping these guards. `scopes-transaction-red.log`
first failed all three transaction cases; green output includes 128 repeated
publication failures, leader-binding failures, quota failures, growth rollback,
and successful charge/appointment retention. Native-target compilation passes,
which is not a native runtime containment result.

[Frame transactions](../src/memory/owned.rs) now accept an explicit maximum frame
count. Every allocation through the transaction counts, including page-table
parents. Ceiling zero rejects before pool/ledger allocation; partial failure drops
all owned frames; commit preserves ownership for later teardown. The existing
constructor keeps its previous unlimited behavior. The QEMU kernel red test
(`frame-ceiling-red.log`) reproduced allocation past a two-frame ceiling. The green
suite passes 95 kernel unit cases, including zero/exact/over-pool ceilings, repeated
rollback, commit/recycle ownership and the default constructor.

The frame increment's four kernel integration executables (`threads`,
`native_supervision`, `process_supervision`, `display_mapping`) pass. Fresh native
images pass exhaustion, 64-cycle process lifecycle, 64-cycle IPC lifecycle and
two-service supervision on each of 1/8/32 CPUs (12 cases). A fresh default-profile
eight-CPU desktop passes. Default/GRUB builds, native GRUB release and kernel test
compilation pass. Transcripts and screenshots are retained in
`target/initator-m5-evidence/frame-ceiling-*`.

At this earlier checkpoint the registry was still standalone. The scheduler
adapter below now wires these reservations and bounded transactions into native
allocation/reclamation. The Ring 3 scope ABI/events and remaining resource domains
are still pending. Service stop/restart jobs must wait for durable scope cleanup
rather than treating foreground exit as full cleanup.

## Native kernel scope binding

[The scheduler adapter](../src/scheduler/scopes.rs) now derives all caller identity
and authority from live kernel process metadata. It exposes kernel Rust entry
points for scope creation, status, close/peek/ack and scoped image construction;
there are no new Ring 3 syscalls. Only init creates sessions. A session leader
with SCOPE_CREATE can create its own service leaves. Scoped spawn checks ownership,
once-only leader binding, parent capability attenuation and role restrictions:
user sessions cannot receive SYSTEM_WRITE, service leaves cannot mint factories,
and Init/Legacy flags are not granted through scoped construction.

[Common process construction](../src/scheduler.rs) reserves process/base-thread and
initial metadata capacity before fallible construction. All five existing launch
paths inherit their parent's scope. Native frame transactions use the remaining
ancestor page budget and include images, BSS, main stacks, private surfaces, TLS
and private page tables. Final page/metadata charging and leader/grant publication
share SCHEDULER; any failure destroys allocations and restores the reservation.
The legacy profile keeps its explicit unscoped/trusted behavior.

[Heap growth](../src/scheduler/heap.rs) caps actual leaves and new parent tables;
failed mapping rolls both back before returning quota. Shrink releases only unmapped
leaf charges after shootdown; empty parent tables remain process-owned. Replacing
the frame ledger adjusts its persistent allocation charge. [Thread construction](../src/scheduler/threads.rs)
reserves a thread, kernel stack and TLS ledger, then charges actual stack/TLS frames.
The TLS ledger allocation is fallible before mapping, replacing an infallible
`split_off` after allocation. The base-thread reservation is retained with process
resources; extra-thread charges persist through zombie/join until actual reaping.

[Reclamation](../src/scheduler/reclaim.rs) releases quota after off-CPU checks,
unmapping, TLB acknowledgement, frame recycling and owned metadata/kernel-stack
release. A process's charge travels with its retired PCB. Consuming a child status
does not complete scope cleanup. [Supervision](../src/scheduler/supervision.rs)
reparents orphans to a live scope controller before init, preserving a live parent
first. Leader/controller death marks scopes closing; a bounded, allocation-free
victim drain stops remaining members without recursive scope traversal. Connected
child termination also notifies the scope registry.

Native boot configures limits from real frame-pool capacity and the existing heap:
128 process reservations, 1024 thread reservations and half the heap for selected
persistent allocations. Recovery floors reserve four processes, eight threads,
up to 8192 pages (at most a quarter of the pool), and a sixteenth of the heap.
Other resource fields are not instrumented yet. These limits are not a complete
kernel-heap/temporary-I/O quota. Normal Initator still launches trusted services in
the system root until its scope-aware native adapter is implemented.

[The kernel integration test](../tests/scopes.rs) first failed because the native
registry was absent (`scopes-kernel-red.log`), then exposed unaccounted heap and
thread allocation (`scopes-kernel-construction.log`, `scopes-kernel-heap.log`). Its
green assertions use real private mappings and resource reclamation: zero/late
metadata and page-budget spawn failure rollback; inherited process limits;
partial-heap parent rollback; thread quota retained through join; 8 KiB and 1 MiB
ELF TLS templates, exact stack/TLS frame deltas, independent template initialization
and TLS-aware quota denial; delayed closure until physical cleanup; immutable user/
session identity; live-manager orphan ownership; two same-user sessions rejecting
each other's factory/control requests; manager death killing its services while
the other session survives. Global frame/selected-allocation counts return to the
initial baseline. This bootloader harness drives kernel contexts on one CPU; it
is not yet a Ring 3 scope-protocol or SMP scoped-grant acceptance test.

The kernel unit suite (95 cases), eight integration executables, default/GRUB builds,
native/default GRUB release images, all rebuilt apps and test compilation pass.
The 66 existing native cases pass on 1/8/32 CPUs, exercising root-scope accounting
through real Ring 3 spawn/heap/thread/exit paths. This compatibility matrix does
not substitute for full scoped-grant syscall tests. All 33 selected unit/control
and shutdown cases also pass across 1/8/32 CPUs, as do SMP stress and the six
FPU/compiler-TLS runs. The fresh default-profile eight-CPU desktop passes its
complete automated interaction assertions; its screenshots are retained under
`target/initator-m5-evidence/scopes-desktop-screenshots/`. Manual host GUI input
and physical hardware remain unverified.

Two existing test defects were reproduced and repaired without extending timeouts.
The shutdown runner sometimes inspected a serial halt record before its final
bytes arrived. A host regression failed on 58 partial prefixes; a newline-aware
parser now waits for a complete record and still rejects complete failure records.
All 79 Python tooling tests pass. The failing 32-CPU pending-launch case and the
remaining shutdown cases pass after the fix. Evidence: `scopes-control-shutdown.log`,
`scopes-halt-parser-red.log`, `scopes-halt-parser-green.log`,
`scopes-pending-launch-rerun.log` and `scopes-control-shutdown-resume.log`.

The SMP probe still treated event bit 32 as invalid, although the existing ABI
assigns it to child readiness. Its zero-timeout wait therefore correctly parked
forever. The same failure was reproduced on the previous, unlinked-scope image
(`scopes-smp-baseline.log`). The probe now tests child/port readiness with finite
waits and derives the unknown bit from EVENT_ALL; the invalid-bit assertion is
preserved. Rebuilding the probe and disk makes the unchanged stress runner pass
on 1/8/32 CPUs (`scopes-smp-stress-green.log`). This is a stale ABI-test migration,
not evidence that the previous image passed. The generated probe binary is
regenerated with its source. No kernel behavior changed for either test repair.

## Session record admission

Each session is now limited to 16 service scope records in the shared 64-slot
registry. This bound includes empty, Closing and Closed records until explicit
acknowledgement; releasing resources alone cannot admit another record. Only init
can create additional session roots. The existing four system-recovery slots are
still reserved independently of this per-session bound. This fixed initial limit
avoids another configurable quota field before the Ring 3 factory ABI is designed.

The new host regression first admitted a seventeenth leaf (`Ok(20)`); the kernel
integration independently reproduced the missing bound (`Ok(28)`). Both now pass,
including a sibling session creating a leaf under pressure, no identity consumption
on denial, cleanup acknowledgement restoring capacity and resource-release-only
not restoring it. The global pressure fixture now fills several independently
admitted sessions and retains its original assertion of exactly four recovery
slots. Host scope tests pass (17 runtime and two compile-fail cases), the expanded
kernel scope integration passes, and the 95 kernel unit cases, both boot-path
builds, native GRUB release and test compilation pass. Evidence:
`scopes-session-admission-{red,green}.log`, `scopes-session-kernel-{red,green}.log`
and `scopes-session-{unit,build-default,build-grub,build-release,no-run}.log`.

Before this admission-only change, all six loader/watchdog latency profiles passed
on 1/8/32 CPUs, with and without dependent cancellation (`scopes-latency.log`).
Each observed watchdog stop had zero delay ticks; the configured two-tick budget
was unchanged. Those GRUB images exercise the preceding native scope binding;
the admission change is covered by the newly rebuilt kernel integration. Neither
validation exposes the still-incomplete Ring 3 scope factory.

## IPC inbox lifetime prerequisite

A new kernel allocator regression exposed a retained allocation in
[IPC closure](../crates/ipc-core/src/lib.rs): closing an endpoint discarded its
messages but kept its entire inbox allocation until its peer closed. Returning
an endpoint's future scope charge at that point would incorrectly claim the
memory was free. Closure now drops the dead endpoint's inbox immediately. The
surviving peer retains its own queued messages, drains them in FIFO order and
receives one durable PeerClosed notification. Binding/rights attenuation semantics
are unchanged; a live endpoint's reserved inbox is retained.

[The test](../tests/ipc.rs) first failed its actual allocated-kernel-byte assertion
(`scopes-ipc-storage-red.log`). It now passes eight alternating explicit-close and
owner-death cycles, with both inboxes full, immediate dead-inbox release, retained
peer FIFO/sender identities, stale-handle denial, single close notification and
exact final heap recovery. The full IPC host suite passes (15 cases). Final-source
checks pass: 95 kernel unit cases, IPC and scope integration executables, both
boot-path builds, test compilation, a fresh native GRUB release/all-app image and
nine real Ring 3 IPC/64-cycle cleanup/two-service supervision cases across
1/8/32 CPUs. Evidence: `scopes-ipc-final-{host,kernel,build-default,build-grub,no-run,image-build,native}.log`.
This repairs allocation lifetime; scope accounting and atomic budget transfer for
endpoint grants remain pending. It does not establish complete IPC isolation.

## Native IPC resource accounting

Native channel construction now reserves two handles and the bounded inbox storage
before allocation. The actual VecDeque capacities are then charged before handle
publication. A failure destroys new endpoints/inboxes before returning quota;
invalid buffers and exhausted limits preserve older objects. Explicit close and
all process-stop paths release endpoint/inbox charges after actual storage release.
The process's charge retains its remaining reservations through deferred physical
reclamation. The legacy desktop remains explicitly unscoped.

[IPC storage snapshots](../crates/ipc-core/src/lib.rs) count allocated capacity,
including live endpoints whose receive rights were removed, rather than just
queued payload bytes. [Atomic charge transfers](../crates/scope-core/src/transfer.rs)
validate source ownership counts, target limits and every affected ancestor before
mutation. Shared ancestors receive only their net change, allowing an inherited
endpoint handoff at a full scope limit. System-to-non-system transfers conservatively
check the recovery ceiling even when the shared system aggregate is unchanged;
existing non-system handoffs remain possible while init consumes recovery capacity.
These checks account resource ownership; endpoint access still depends on the
separately authenticated process key, explicit grant and attenuated rights.

[Construction](../src/scheduler.rs) validates the whole proposed endpoint grant,
finishes fallible image/mapping work and validates receiving quota under SCHEDULER
before committing ownership. It then moves the endpoint charges onto the new PCB
without another allocation or scheduling point. Failed construction leaves source
handles/charges intact and removes unpublished leader appointments. The reservation
commit error returns its guard so callers can roll back real resources before
returning quota. A trusted kernel-only scoped constructor now accepts explicit
bindings; this does not expose a Ring 3 scope factory.

The root bounds live endpoints at 256, reserving 16 for recovery. A separate
physical-object gate leaves eight of the 128 channel slots unavailable to ordinary
scopes: a half-closed channel still occupies a slot with just one live endpoint.
Ordinary pair creation and system-to-ordinary grant admission both honor this
floor. The grant gate conservatively counts all current channels; it may reject
a grant while trusted system channels occupy the recovery region. Endpoint charges
do not include the shared bounded registry's retained slot-vector infrastructure;
selected persistent allocations are still distinct from complete kernel-heap or
temporary-I/O isolation.

The native regression first allocated a pair in a scope limited to one handle
(`scopes-ipc-budget-red.log`). Expanded kernel assertions cover handle and inbox-byte
exhaustion before allocation; repeated failed scoped grant construction restoring
frames, counters and leader state; a successful cross-scope endpoint move;
inherited-scope moves at full handle quota; close/death releasing the correct
owner's charge; 120 half-closed channels still leaving eight usable init slots;
and grant admission refusing to consume recovery slots. Final cleanup restores
the exact initial global frame and selected-resource counters. This remains a
single-CPU kernel-context scope test; real Ring 3 scope-grant/SMP tests await the ABI.

The host suites pass: 23 scope runtime tests plus two compile-fail checks, and
16 IPC runtime tests. Transfer tests include shared-ancestor and cross-session
pressure, recovery admission, arithmetic/boundary failure atomicity and guard
retention on failed commit. API-absence red checks are recorded in
`scopes-transfer-red.log`, `scopes-transfer-commit-guard-red.log` and
`scopes-ipc-storage-query-red.log` (and the scoped constructor API check in
`scopes-ipc-grant-red.log`); green host transcripts are
`scopes-ipc-account-host-{scopes,ipc}.log`.

Both boot-path builds, native/default GRUB release images, all rebuilt apps, test
compilation, 95 kernel unit cases and all eight relevant integration executables
pass. The fresh native image passes all 66 boot/supervision/principal/capability/
filesystem cases on 1/8/32 CPUs; the fresh default image passes the complete
eight-CPU desktop interaction suite. The final commit-error guard refinement then
passes rebuilt scope/IPC integration and kernel unit tests, both builds, native
release/all-app image and nine IPC/lifecycle/two-service runs. Its image also passes
33 readiness/control/shutdown cases and all six isolated loader/watchdog latency
profiles across 1/8/32 CPUs. Every latency run observes zero watchdog-delay ticks;
no acceptance timeout/budget was increased. No project warning or required runtime
failure remains in these selected checks; bootimage still reports its existing
unused rustc-abi target-field warning.

Transcripts: `scopes-ipc-account-{unit,integration,native,desktop}.log`,
`scopes-ipc-account-final-{kernel,build-default,build-grub,no-run,build-native,native}.log`,
`scopes-ipc-account-control-shutdown.log`, `scopes-ipc-account-latency.log` and
`scopes-ipc-account-tooling.log` (79 Python cases). The tested desktop screenshots
are retained under `target/initator-m5-evidence/scopes-ipc-account-screenshots/`;
the repository's generic screenshots are restored. A screenshot cannot prove
quota ownership or cleanup, so allocator assertions, durable-state assertions and
serial transcripts are the primary evidence. Manual host GUI/physical hardware
checks remain unverified. Normal Initator launches still use the trusted system
root; this checkpoint does not complete M5 or justify opening its milestone PR.

## Descriptor and I/O resource leases

[Resource leases](../src/scheduler/resources.rs) now retain independent charges
through work outside the kernel entry gate. SCHEDULER owns a fixed boot-allocated
8192-record pool and release queue, with 64 records reserved for the system root.
Records use nonwrapping serial identities and cannot be reused while their release
is queued. A destructor publishes exactly one key without allocating or acquiring
SCHEDULER; a capacity-sized queue therefore accommodates every outstanding lease.
Scheduler entry/idle reclamation and subsequent resource admission collect releases
under SCHEDULER. Collection has a fixed record-count bound because producers cannot
create replacement leases while that lock is held. Leases can outlive their PCB.
The shared table/free lists/queue are bounded boot infrastructure, allocated before
native userspace publication; their final queue/Arc allocation is a fatal boot
heap prerequisite. No hard IRQ creates or destroys these leases.

[Descriptor construction](../src/fs/handles.rs) reserves a descriptor and its path
allocation before constructing/publishing an OpenFile or performing disk effects.
The normalized path is moved into the object, replacing an extra clone. The open
file's lease is its last field, so path storage is released before its release key
is published. Shared Arc references retain that lease through concurrent close or
blocked I/O. Object allocation now uses fallible Arc::try_new; the existing pinned
nightly needs the explicit allocator_api feature for it. No compiler, dependency,
target or linker configuration was updated.

Native boot caps descriptors at 4096 and leaves 32 for the system root. The existing
1024-number per-process bound is preserved. Slot reservation still precedes
create/truncate; failed opens restore only their own slot and retain temporary
charges until collection. Read/write reserve the bounded owned buffer before
allocation, then charge any larger actual capacity before I/O. Locals drop buffer
storage before the lease, including allocation/copy/I/O failure. Filesystem/open-file
locks never call back into SCHEDULER. The dispatcher and kernel integration harness
use the same validated Rust descriptor facades; syscall numbers, record layouts,
generic errors and existing SDK wrappers are unchanged. Legacy boot returns
explicitly unscoped leases.

The new [scoped file integration](../tests/scoped_files.rs) first returned descriptor
3 for a zero-descriptor scope's truncating open (`scopes-fd-quota-red.log`). It now
checks repeated create/truncate denial without disk effects; 16 failed/successful
open/read/close/reuse cycles; invalid buffers preserving cursor position; I/O-byte
exhaustion preserving cursor and file contents; zero-length progress without
allocating a buffer; and close/exit cleanup. A shared leased object retained beyond
process/frame/stack reclamation keeps its scope Closing until its final reference
drops and collection runs. Global counters then return exactly to baseline.
The native 4064/32 recovery admission check uses unpublished aggregate reservations,
not a claim that 4064 real files were opened. Three kernel unit cases test queue
saturation/reuse, release while SCHEDULER is already locked without heap allocation,
reserved record availability, stale identities, failed growth, serial exhaustion
with cleanup and zero-sized reservations. Their initial missing-pool interface
failure is `scopes-resource-pool-red.log`.

The new integration explicitly formats only the disposable 16 MiB secondary-master
fixture `tests/fixtures/disk-fat32.img`; serialize it with ATA/FAT32 fixture tests.
It runs kernel-side contexts on one CPU. The retained-object test models the Arc
lifetime used by blocked I/O; it does not execute a killed scoped Ring 3 I/O thread.
Full scoped Ring 3/SMP I/O cancellation/quiescence tests still require the grant ABI.
Descriptor Arc/slot-vector headers remain bounded infrastructure rather than
selected-byte charges. Cwd/path-copy metadata, old path APIs, image/argument buffers,
directory listing and internal FAT allocations still require budget/cancellation
work, as do stable home/tmp grants. This is not complete filesystem isolation.

Validation passes: 98 kernel unit cases, nine integration executables (including
scoped_files), both boot-path builds, test compilation, FAT32's two real-disk
integration cases and fresh native/default GRUB release images with all apps rebuilt.
The native image passes all 66 existing boot/supervision/principal/capability/system-
file cases on 1/8/32 CPUs. The default image passes the descriptor probe on all
three CPU profiles and the complete eight-CPU desktop interaction suite. All
15 shutdown cases pass, including pending-loader cancellation; the six isolated
loader/watchdog profiles report zero watchdog-delay ticks at unchanged budgets.

[The native file runner](../e2e/init_file_io.py) additionally installs the existing
zfileio probe as a finite Restart=never service beside Glow. All three CPU profiles
pass actual native resource-leased opens, process exit/kill cleanup, 1024-descriptor
limits and eight shared-cursor workers delivering every record exactly once.
The runner requires PID 1 bring-up, distinct service identities, the full probe
marker, that exact service's successful completion and surviving sibling progress.
Six host regressions first failed on the missing runner-evidence interface, then
pass for fixture shape, incomplete records, foreign completions, failure status,
duplicate/reused identities and sibling termination. The complete Python suite
passes (85 cases). This native test uses the trusted system scope; it does not
replace future scoped user-session Ring 3 acceptance.

Evidence is retained under `target/initator-m5-evidence/`:
`scopes-fd-{unit,integration,fat32,native,file-io,desktop,shutdown,latency}.log`,
`scopes-fd-supervised-file-io.log`, `scopes-fd-native-harness-{red,green}.log` and
`scopes-fd-final-tooling.log`. Builds use `scopes-fd-{build-default,build-grub,no-run}.log`
and `scopes-fd-final-{native-build,desktop-build}.log`. Tested desktop screenshots
are archived in `scopes-fd-screenshots/`; generic tracked images are restored.
Quota ownership/lifetime evidence is the executable allocator/state assertions
and serial logs; screenshots cannot demonstrate it. Manual host GUI and physical
hardware checks remain unverified. The earlier missing-doc warning on the new
integration entry was corrected; only the pre-existing bootloader rustc-abi warning
remains. M5 is still incomplete and no milestone PR is opened.

## Owned syscall and executable snapshots

[ResourceBuffer](../src/scheduler/resources.rs) now couples fixed-length owned
bytes to their native reservation. It reserves before allocation, charges actual
capacity, exposes only a slice and drops byte storage before queuing release.
Descriptor I/O uses this type instead of repeating the lease/buffer drop-order
pattern. It also covers console-write copies, old whole-file path read/write,
explicit mapped-image spawn snapshots and disk executable staging. Allocation or
scope-budget failure returns existing generic errors before I/O, user-output or
child-publication effects. This does not impose a new arbitrary transfer-size cap;
native scope budgets and existing per-operation bounds remain authoritative.

[Disk loading](../src/scheduler.rs) now queries file size, releases the filesystem
lock to reserve owned storage, then reacquires it to recheck size and read a single
consistent snapshot. A changed size/short read fails instead of publishing a
partial image. No scheduler reservation occurs under a filesystem lock. The
initial boot image alone uses an explicit unscoped buffer: the scheduler verifies
no current process, no published/retired process or thread, and the untouched
initial PID sequence. Later no-caller/dead-caller loads cannot claim that exception.
[Boot entry](../src/entry_point.rs) drops staging after construction copies private
image/TLS storage, before its nonreturning switch. The kernel integration confirms
actual heap release and preserved executable bytes after staging is dropped.

[Launch snapshots](../src/userspace/launch.rs) charge copied pointer/length records,
String vector storage and payload, argv/envp reference arrays and binding metadata.
Lengths/counts and the combined existing startup bound are checked on owned records
before allocating payload. Larger actual capacities require additional admission.
Each owning vector/string is destroyed before its reservation. Version-1 environment
launch and version-2/3 bound/image launch share the same snapshot path. Zero-length
vectors keep their existing empty-pointer semantics. Complete mapped usercopy,
UTF-8/NUL checks and endpoint-constructor authority checks remain separate gates.
Kernel Rust snapshot callbacks/facades are for constructors and integration; the
userspace dispatcher still applies mechanism capability checks. No syscall number,
wire record, SDK wrapper or startup byte/count limit changed.

The expanded [kernel integration](../tests/scoped_files.rs) first staged an image
with no byte budget (`scopes-snapshot-budget-red.log`), then read nine file bytes
under the same exhausted budget (`scopes-snapshot-path-red.log`). Both now deny
without changing output/file bytes. The argument regression initially invoked its
launch callback (returned 77) without any allowance for copied records/strings
(`scopes-snapshot-vectors-red.log`); zero and record-only budgets now prevent the
callback. A valid snapshot succeeds with literal argv and no residual charges.
A 64 KiB mapped-image snapshot fails while 40 KiB remains available, without
publishing a child. A control directly constructs and reclaims that same image
within the remaining allowance, proving the denial concerns the additional
snapshot rather than target construction. Malformed vector/binding counts fail
before the callback. A real staged buffer retained after PCB/frame release
keeps its scope Closing until the buffer drops and collection runs; global counters
return exactly to baseline. These are still kernel-context scope tests, not the
pending Ring 3 scope-grant ABI acceptance.

C-string/path/cwd storage, directory output/entries, internal FAT allocations,
filesystem grants and cancellation still need work. Path-based executable loading
also needs the same grant/read-authorization boundary as ordinary reads; the current
SPAWN mechanism domain alone is not a filesystem sandbox. Normal Initator services
still share the trusted system root. Complete isolation and Nest remain unfinished.

Both boot-path builds, test compilation, 98 kernel unit cases and all nine
integration executables pass, as do the final expanded scoped-file assertions.
Fresh native/default GRUB release images rebuild all apps. The native image passes
66 boot/supervision/principal/capability/system-file cases, three supervised
filesystem runs and three literal-vector unit runs across 1/8/32 CPUs. The default
image passes 15 image/environment/large-file/FPU/compiler-TLS cases across those
profiles and the full eight-CPU desktop interaction suite. All 15 shutdown cases
pass. Six isolated loader/watchdog profiles (normal/dependent cancellation at
1/8/32 CPUs) report zero watchdog-delay ticks at unchanged acceptance budgets.
All 85 Python tooling cases and documentation checks pass. No required check in
this selected scope remains failing; the existing bootloader rustc-abi warning
persists. Manual host GUI and physical hardware remain unverified.

Evidence: `scopes-snapshot-{budget-red,path-red,vectors-red,full-green,final-integration}.log`,
`scopes-snapshot-{unit,integration,native,native-vectors-files,capabilities,desktop,shutdown,latency,tooling}.log`
and `scopes-snapshot-{build-default,build-grub,no-run,native-build,desktop-build}.log`
in `target/initator-m5-evidence/`. The final integration refinement is test-only;
its constructor control and malformed-count cases pass after the broader matrix.
Tested desktop screenshots are archived in `scopes-snapshot-screenshots/`, with
generic tracked images restored. Kernel allocator/state assertions and serial logs
prove the snapshot accounting; screenshots do not demonstrate ownership or cleanup.
The original disposable FAT fixture is restored after serialized kernel tests.
This checkpoint does not complete M5 and no milestone PR is opened.

## Path and cwd ownership

[ResourceString](../src/scheduler/resources.rs) couples UTF-8 storage with a scope
lease and exposes bounded append operations, not a growable String reference.
Capacity is reserved fallibly and charged before allocation; an append beyond
capacity returns Limit without allocating or changing bytes. Existing UTF-8 bytes
can be normalized without growth. Initial process cwd storage uses an explicit
already-accounted adapter because common construction retains that allocation's
charge. Later chdir values retain independent leases through PCB reclamation.

[C-string capture](../src/userspace/usercopy.rs) now reserves the bounded path
buffer before copying. Its 4096-byte capacity remains charged even when the NUL
terminator shortens the visible slice. Copies remain page bounded; a terminator
at the final byte of a mapped page does not touch the following unmapped page.
[Path normalization](../src/fs/handles.rs) takes a charged cwd snapshot only for
relative paths and reserves all output capacity before taking the filesystem
lock. Checked appends preserve the existing 4095-byte normalized bound and still
validate traversed directories canceled by `..`. Recursive mkdir alone permits
missing intermediate components. FAT spelling, short aliases and Unicode remain
filesystem mechanisms, not access grants.

Cwd copying allocates outside SCHEDULER and then copies coherent metadata under
that lock, failing safely if a concurrent longer cwd no longer fits. getcwd adds
its NUL terminator within precharged capacity and validates/copies the complete
advertised output. Open moves its charged normalized path into OpenFile and adds
the descriptor reservation to that same lease, avoiding duplicate path charges
or an extra persistent lease per descriptor. Reclamation collects releases again
after dropping retired PCBs, so newly released cwd leases complete only after
actual allocation destruction. Service locks never allocate/grow these strings
or acquire scheduler reservations; internal FAT metadata allocations remain a
separate unfinished boundary.

The regression first returned two cwd bytes with a fully exhausted native budget
(`scopes-path-budget-red.log`). It now denies repeated getcwd/chdir attempts while
preserving output and cwd. Further assertions cover retained cwd charges, failed
`/MISSING/..` traversal, exact cleanup, bounded multibyte append failure without
heap growth, a final-page NUL terminator and a missing terminator across the full
4096-byte bound. The prior I/O-byte exhaustion test now admits temporary path
setup first, then explicitly reserves all remaining capacity before exercising
its unchanged read/write/cursor/file-preservation assertions. Its first run after
path charging correctly rejected setup (`scopes-path-first-green.log`); this test
migration preserves the intended exhausted-I/O condition. Expanded scoped-file
and existing scope integrations pass (`scopes-path-green.log`).

Both boot-path builds, test compilation, 98 kernel unit cases and all nine kernel
integration executables pass, along with fresh native/default GRUB release images
and all rebuilt apps. Across 1/8/32 CPUs the native image passes all 66 existing
boot/supervision/principal/capability/system-file cases. Fifteen further cases pass:
native supervised file I/O, literal-vector activation, legacy file I/O and Unicode
name/environment checks on each profile. The complete eight-CPU desktop regression
and all 15 shutdown cases pass. Six isolated loader/watchdog profiles report zero
watchdog-delay ticks with unchanged acceptance budgets. All 85 Python tooling
cases and documentation checks pass. No required check in this selected scope
remains failing; bootimage retains its pre-existing rustc-abi warning. Manual host
GUI and physical hardware checks remain unverified.

Evidence in `target/initator-m5-evidence/`:
`scopes-path-{budget-red,first-green,green,unit,integration,native,files-vectors,desktop,shutdown,latency,tooling}.log`
and `scopes-path-{build-default,build-grub,no-run,native-build,desktop-build}.log`.
Tested screenshots are retained in `scopes-path-screenshots/`; the generic tracked
images and disposable FAT fixture are restored after testing. Quota/cwd lifetime
proof comes from the executable counter, allocation and data assertions rather
than a screenshot. At that checkpoint, listings/internal FAT allocations, grants,
I/O cancellation/quiescence, socket/audio budgets and the Ring 3 scope/session
factory remained unfinished; the next section closes the listing allocation gap. Normal services still share the trusted system root; Nest does
not run yet, and this is not a completed filesystem sandbox or M5 milestone.

## Directory enumeration and output admission

Native directory syscalls 5 (short aliases) and 33 (full names) now use
[`Fat32::visit_dir`](../src/fat32.rs) rather than collecting all entries. The
scanner keeps at most twenty VFAT slot locations, 260 UTF-16 units, 765 UTF-8
bytes and one sector in bounded stack scratch. Names are borrowed only for the
callback. Directory path traversal and short-name matching no longer allocate
component/name strings. Each visited directory sector is read once per scan.
The collecting `list_dir` helper remains for trusted kernel callers/tests; it
is no longer used by either directory syscall.

The [kernel filesystem adapter](../src/fs/handles.rs) first counts required
output, releases the filesystem lock, admits a fallible `ResourceBuffer`, then
fills that buffer in a second locked pass. It never takes SCHEDULER while holding
FS. The buffer and charge survive through usercopy. A changed required size or
an I/O/callback error fails before copying any listing prefix. Equal-sized
changes produce the second pass's coherent listing; there is no durable snapshot
or cursor ABI. Zero-capacity full-name queries use no listing-sized allocation.
The existing short-name record and aligned UTF-8 record formats remain unchanged.
The public Rust helpers exported from `fs` are kernel harness interfaces; the
syscall dispatcher still applies capability filtering for Ring 3 callers.

Observed red checks, recorded under `target/initator-m5-evidence/`:

- `directory-budget-red-small.log`: the caller had only 8 KiB spare, but a 9,920-byte
  listing succeeded instead of returning the existing error sentinel.
- `directory-scratch-red.log`: the scanner retained 120 extra heap bytes during
  its callback rather than using bounded scratch.
- `directory-sector-red.log`: a single-sector listing performed five reads rather
  than one. The revised scanner caches that sector until its slots are consumed.

The first test fixture (`directory-budget-red.log`) timed out during creation of
many colliding aliases, before reaching the quota assertion. It is not a red
behavior proof. The replacement uses forty unique-prefix, long Unicode names;
it still spans multiple directory clusters and exceeds the same 8 KiB spare
budget. The timeout and production limits were not increased.

[The scoped-files test](../tests/scoped_files.rs) checks repeated quota denial
without modified output, a successful size query under that pressure, exact
long-name record bytes/padding, short listing caps including zero, invalid and
cross-page/read-only/overflow buffers, missing/non-directory paths, and exact
scope cleanup after reaping. The RAM-backed kernel test asserts no heap growth
while visiting decoded names or resolving a long directory path, callback error
propagation, short-name fallback and sector read counts. Host VFAT tests cover
maximum 255-unit names, the full 765-byte UTF-8 bound, surrogate pairs, padding,
checksum/order errors and repeated decoder completion.

Validation run for this checkpoint:

- Default and GRUB debug builds; both complete release disk builders (native
  `target/init-m5-directory.img` and default `target/init-m5-directory-desktop.img`).
- 99 kernel unit cases and ten integration executables: `display_mapping`,
  `fat32`, `ipc`, `keyboard_timer`, `native_supervision`, `process_supervision`,
  `scoped_files`, `scopes`, `stdio`, `threads`. The FAT wrapper's underlying
  `cargo test --test fat32` was included directly in this serialized run.
- 78 Ring 3 runs: all 22 `e2e/init.py` cases, native `init_file_io.py`, unit
  `vectors`, and default `capabilities.py` `names`/`files`, each at 1/8/32 CPUs.
- Full default desktop at eight CPUs; six isolated 8 MiB/s loader/watchdog
  normal/cancellation checks at 1/8/32 CPUs. Every watchdog delay was zero ticks.
- Three host VFAT cases, 85 Python tooling cases, documentation/link checks and
  whitespace checks. No timeout or watchdog budget was increased.

Transcripts are `directory-*.log` in `target/initator-m5-evidence/`. Desktop
screenshots from the tested build are archived in `directory-screenshots/` and
the generic tracked images are restored. The quota proof is the executable
allocation, record-byte and cleanup assertions; a screenshot cannot establish
those invariants. Final import/format cleanup changes no executable behavior.
Manual host GUI and physical-device checks remain unverified. The known debug
heap-stress timeout is unrelated and was not rerun or claimed as passing.

This closes the listing allocation gap. Internal FAT file read/write, path
resolution and mutation scratch allocations still require work. It does not
provide filesystem grants, cancellation or complete hostile-service isolation.

## Bounded FAT file I/O and borrowed path traversal

[`fat32.rs`](../src/fat32.rs) no longer allocates copied path-component vectors
or a final filename string during lookup. Parent resolution borrows the caller's
owned path and validates all components before disk lookup, preserving invalid-name
error ordering. Recursive directory creation also walks borrowed components;
creating names still has separate metadata allocations described below.

Whole-file and offset reads fill the already admitted caller buffer directly.
The checked cluster-subrange helper uses only partial-sector scratch and retains
bulk device reads for aligned middle sectors. Range arithmetic and the complete
requested disk extent are checked before I/O. Zero-length cluster subranges
perform no sector transfers; path APIs still validate file metadata. Small reads
no longer fetch all sectors of a cluster.

Whole-file replacement writes from the admitted source slice without constructing
a second cluster buffer. Cluster allocation clears sectors with a fixed zero
buffer. Offset writes validate the existing chain using its length/tail rather
than collecting every cluster in a vector, build any growth as a separate chain,
and patch affected sectors with fixed scratch. Growth is linked only after data
writes, then the directory size is published. Failure paths free staged growth
and restore the old tail link as before. This preserves the existing error model:
in-place overwrites are not transactional, persistent device errors can defeat
rollback, and this adds no crash consistency, fsync or durability guarantee.

Observed red checks under `target/initator-m5-evidence/`:

- `fat-read-metadata-red.log`: copied path metadata raised live heap usage during
  mock disk I/O above the pre-operation baseline.
- `fat-read-buffer-red.log`: after borrowing paths, offset-read cluster scratch
  still raised usage above that baseline.
- `fat-read-mkdir-red.log`: walking an existing directory for recursive mkdir
  still copied its components.
- `fat-write-buffer-red.log`: replacement/cluster-zero scratch still allocated.
- `fat-range-buffer-red.log`: the per-file cluster vector still allocated during
  an offset write. Each preceding stage was removed before the next red run.

The RAM device checks heap usage on reads and writes; operation data/output are
allocated before enabling that check. Tests cover aligned and unaligned ranges,
sector/cluster boundaries, EOF, empty requests, caller-buffer guards, long Unicode
paths, complete path validation, gap zeros, cross-sector patching, replacement
and truncation. Cluster-subrange tests assert exact sector counts and reject
out-of-range/overflow requests without I/O. Repeated injected publication and
gap-read failures assert old contents/size, tail termination and exact free-cluster
recovery. Existing disk-full/replacement and unrelated-cluster write assertions
remain in the suite.

Validation run for this checkpoint:

- Default and GRUB debug builds and both complete release disk builders:
  `target/init-m5-fat-io.img` (native) and `target/init-m5-fat-io-desktop.img`.
- 102 kernel unit cases; ten integration executables (`display_mapping`, `fat32`,
  `ipc`, `keyboard_timer`, `native_supervision`, `process_supervision`,
  `scoped_files`, `scopes`, `stdio`, `threads`). FAT fixture checks ran serially;
  the wrapper's underlying `cargo test --test fat32` was included directly.
- 81 real Ring 3 runs: the 22 native init cases, native `init_file_io.py`, unit
  `vectors`, default `file_io.py`, and capability `names`/`files`, each at 1/8/32
  CPUs. These include shared-cursor workers, large files and host-mtools name
  interoperability, not just kernel-context calls.
- Full default desktop at eight CPUs; six isolated 8 MiB/s loader/watchdog checks
  (normal and cancellation at 1/8/32 CPUs), all with zero watchdog delay ticks.
- Documentation/link checks and `git diff --check`. Unchanged host core/tooling
  suites were not rerun. No timeout or latency budget was increased.

Logs are `fat-*-red.log` and `fat-io-*.log` under
`target/initator-m5-evidence/`; tested screenshots are in `fat-io-screenshots/`.
The generated tracked screenshots and disposable FAT image are restored. The
allocation/rollback proof comes from executable assertions and counters rather
than a screenshot. Final source edits after the release builds are comments only.
Manual host GUI and physical hardware remain unverified; the existing debug heap
stress timeout was not rerun or claimed as passing.

This removes path-copy, read, replacement and offset-write heap scratch from the
native syscall paths. Creation/alias/VFAT slot metadata still allocates and is the
next filesystem accounting gap. Filesystem grants, I/O cancellation/quiescence,
socket/audio budgets, the Ring 3 scope factory, scope-aware Initator cleanup and
Nest remain unfinished; this is not a completed M5 or hostile-service sandbox.

## Bounded namespace creation metadata and failure cleanup

The native FAT namespace operations now use fixed scratch alongside the bounded
I/O paths above. [`fat32.rs`](../src/fat32.rs) cleans only the eight base and three
extension bytes needed for an alias, formats its bounded numeric suffix on the
stack, reserves at most 21 directory locations, and installs at most twenty VFAT
records plus one short record. Deletion copies at most twenty linked locations
without allocating a vector. [`lfn.rs`](../src/fat32/lfn.rs) encodes into caller
storage using at most 255 UTF-16 units; insufficient output or invalid names fail
before any output record is changed. Host tests retain a collecting convenience
wrapper; production uses the bounded interface.

The collecting `list_dir`/`DirEntry::file_name` helpers remain trusted kernel
conveniences and still allocate. Neither directory syscall uses them. This
checkpoint removes transient FAT heap allocations from native lookup, read,
write, create and listing paths; syscall staging/path ownership and persistent
file objects still use their existing admission/count limits. It does not claim
that every kernel subsystem allocation has byte accounting. The descriptor object
and slot-table bounds, socket/audio ownership, filesystem grants and I/O lifetime
rules must remain part of the full M5 isolation audit.

Red evidence under `target/initator-m5-evidence/`:

- `fat-names-allocation-red.log`: creating a long-name directory exceeded the
  RAM device's pre-operation heap baseline during I/O.
- `fat-names-directory-cleanup-red.log`: a failed directory initialization left
  117 free clusters instead of 118. `make_dir` now reclaims its unpublished
  cluster on every post-allocation initialization or publication error, including
  failures writing `.` or `..`. Completed ancestors of recursive mkdir remain;
  the operation is not a multi-directory transaction.

Tests keep operation inputs allocated before enabling the heap guard, then
exercise Unicode directory creation, twelve colliding aliases including the
single-to-double-digit suffix transition, directory growth, maximum 255-unit
names, repeated delete/recreate and complete borrowed enumeration. Slot counts
zero/22 fail without I/O. Repeated dot/dotdot failures preserve exact free space,
then successful retry and idempotent mkdir succeed. A publication failure spanning
the last old slot and a newly allocated directory cluster reclaims growth,
preserves existing files and permits retry. Host encoding cases check untouched
output on errors, record order, padding/checksum and the maximum encoding bound.

The first extra growth fixture (`fat-names-growth-fixture-failure.log`) targeted
the next-allocation hint, which may still identify an occupied cluster; its
injected error did not fire. The corrected fixture resolves the actual next free
cluster before injecting the same failure. Expected errors, free-space checks
and timeouts were not weakened. Persistent device errors may still prevent
rollback; no FAT journal, crash consistency or new unlink syscall is introduced.

Validation run for this checkpoint:

- Default and GRUB debug builds and both complete release disk builders:
  `target/init-m5-fat-names.img` and `target/init-m5-fat-names-desktop.img`.
- 105 kernel unit cases; ten integration executables (`display_mapping`, `fat32`,
  `ipc`, `keyboard_timer`, `native_supervision`, `process_supervision`,
  `scoped_files`, `scopes`, `stdio`, `threads`), serialized around writable
  fixtures. The FAT wrapper's underlying test ran directly in that selection.
- 81 real Ring 3 runs: all 22 native init cases, native `init_file_io.py`, unit
  `vectors`, default `file_io.py`, and capability `names`/`files`, each at 1/8/32
  CPUs. This includes host-mtools interoperability and the native worker probe.
- Full default desktop at eight CPUs; six isolated 8 MiB/s loader/watchdog
  normal/cancellation runs at 1/8/32 CPUs, all with zero watchdog delay ticks.
- Four host VFAT cases, documentation/link and whitespace checks. No timeout or
  latency budget changed. Unchanged host core/tooling suites were not rerun.

Transcripts are `fat-names-*.log` under `target/initator-m5-evidence/`; screenshots
from the tested desktop are in `fat-names-screenshots/`. Generated tracked images
and the disposable FAT fixture are restored. Kernel allocation/free-space
assertions establish the accounting and recovery properties; a screenshot cannot.
Source additions after the release builds were unit tests only. Manual host GUI
and physical hardware remain unverified, and the existing debug heap-stress
baseline was not rerun or claimed as passing.

Filesystem grants must cover path traversal, directory listing and executable
loads as well as final read/write effects. In particular, the current path helper
checks intermediate directories, so guarding only the final open is insufficient.
The Ring 3 scope factory stays closed until those permissions and remaining
resource/lifetime gates are enforced. Nest and scoped Initator restart/cleanup
integration remain unfinished; no M5 PR is claimed by these kernel prerequisites.

## Enforced kernel directory grants

[`fs/access.rs`](../src/fs/access.rs) adds READ/WRITE upper bounds for up to eight
existing directory roots. Requests resolve to opaque FAT directory identities,
including long/short aliases, rather than relying on string prefixes. Root
creation validates directory geometry and a bounded, acyclic parent chain to the
mounted root. Subtree and ancestor checks use those validated parent links.
There is one immutable boot mount; no userspace directory deletion, rename or
remount can recycle identities. Those future APIs require explicit grant
invalidation/generation handling before introduction. Filesystem metadata remains
trusted kernel-maintained data; this is not resistance to an attacker replacing
the whole boot disk.

Each PCB owns a fixed grant set in the existing bounded process table.
`scheduler::restrict_filesystem` is a kernel-only adapter: it snapshots the live
caller, excludes PID 1 and sibling/unreaped threads, validates roots/rights under
FS, then revalidates and publishes the complete subset under SCHEDULER. No
scheduler lock is acquired under FS. Empty grants deny all file access. Existing
children retain copied rights; future ordinary children inherit the current set.
Kernel scoped construction across a session boundary defaults to no access.
Initial grants must still be added atomically to the future public scope factory.
The kernel chooses no home/tmp names or login policy.

The [filesystem adapter](../src/fs/handles.rs) applies these bounds to path
traversal, cwd changes, open/create/truncate, reads/writes, metadata seeks and both
directory formats, including both passes of staged listings. Search can traverse
ancestors of a grant without granting a directory listing there. Write-only roots
do not allow reads or listings. Existing descriptors are checked against current
authority on each operation, so opening before reduction cannot retain outside
access. Cleanup/close stays available. Recursive mkdir is now routed through the
same adapter. Existing protected-system-root checks remain additional bounds.

[Disk executable loading](../src/scheduler.rs) checks FILE_READ and directory
rights before metadata access and again for the data pass. This closes the old
SPAWN-only file-loading bypass across path-based spawn routes. Owned-image launch
still needs only SPAWN and the caller's valid owned bytes. No syscall numbers or
principal wire layout changed; there is no Ring 3 grant configuration API yet.

Observed red evidence in `target/initator-m5-evidence/`:

- `fs-grants-policy-red.log`: a requested write outside the allowed rights still
  succeeded before the policy checks were implemented.
- `fs-grants-native-red.log`: an already-open outside descriptor returned three
  bytes after filesystem attenuation instead of failing without output.
- `fs-grants-malformed-root-red.log`: an invalid zero-cluster directory could mint
  a root grant; root sealing now rejects invalid geometry and cyclic parents.
- `fs-grants-spawn-read-red.log`: the strengthened real Ring 3 probe, booted with
  the previous kernel image, published a path-spawned child without FILE_READ.
  The current kernel passes those denials while owned-image spawning succeeds.

The first full native run (`fs-grants-pre-migration.log`) exposed the previous
probe's obsolete SPAWN-only positive path launch. The updated probe preserves
SPAWN-only inheritance through an owned image, asserts all path-spawn denials,
and adds positive path inheritance with SPAWN plus FILE_READ. It was run against
the old kernel to demonstrate the missing check, then rebuilt with the current
kernel. No timeout, expected child exit or existing cleanup assertion was relaxed.

Kernel policy tests cover aliases, subset/expansion, empty/oversized grants,
readonly/write-only roots, nested unions, malformed links and allocation-free
checks. The scoped-files integration exercises old descriptors, whole-file I/O,
listings, traversal canceled by `..`, mkdir effects, executable reads, inherited
children, further reduction, capability bounds and exact scope resource recovery.
These are kernel-context restricted-process tests. Real Ring 3 probes exercise the
read/spawn fix and compatibility, but do not yet configure restricted directory
grants through the still-closed scope/session factory. That native matrix, initial
assignment, I/O quiescence/cancellation, socket/audio accounting, scope-aware
Initator stop/restart and Nest remain required before M5 delivery.

Validation for the directory-grant checkpoint:

- Default and GRUB debug builds; both complete release builders produced
  `target/init-m5-fs-grants.img` and `target/init-m5-fs-grants-desktop.img`.
- 108 kernel unit cases and ten integration executables (`display_mapping`,
  `fat32`, `ipc`, `keyboard_timer`, `native_supervision`, `process_supervision`,
  `scoped_files`, `scopes`, `stdio`, `threads`), serialized around writable
  fixtures. These include restricted kernel-context grant tests, not a Ring 3
  scope-factory acceptance claim.
- 81 real Ring 3 runs: all 22 native init cases, native `init_file_io.py`, unit
  `vectors`, default `file_io.py`, and capability `names`/`files`, each at 1/8/32
  CPUs. The migrated capability probe checks file-read permission on every path
  spawn route, positive owned-image inheritance and positive read+spawn paths.
- Full default desktop at eight CPUs; six isolated 8 MiB/s loader/watchdog
  normal/cancellation checks at 1/8/32 CPUs, all with zero watchdog delay ticks.
- Documentation/link and whitespace checks. Unchanged host policy/tooling suites
  were not rerun, and no timeout or latency budget was increased.

Evidence is retained as `fs-grants-*.log` under `target/initator-m5-evidence/`.
The successful focused capability run at one CPU was included once in the full
matrix rather than needlessly repeated. Probe binaries and read-only metadata
were regenerated together with `tools/build-user.sh`; metadata bytes remained
unchanged. Final source edits after the builds are documentation only. Tested
screenshots are in `fs-grants-screenshots/`; tracked generic screenshots and the
FAT fixture are restored. Permission/resource assertions establish these
properties; screenshots demonstrate desktop compatibility only. Manual host GUI
and physical hardware remain unverified. The known debug heap-stress timeout
was not rerun or claimed as passing.

## Scope integration invariants for the next implementation

- Keep the initial hierarchy bounded: system/session roots and service scopes;
  arbitrary recursive scope creation is not needed. User identity/session selection
  requires kernel-derived authority; ordinary spawn inherits its scope and cannot
  mint identities or enlarge grants. Session managers need explicit factory rights,
  not inherited ambient scope creation.
- Track resources through real reclamation, including unpublished reservations,
  per-process frame ledgers, private surfaces, thread stacks/TLS, IPC factories,
  files, sockets and audio ownership. Zombie status, stopped execution and physically
  released resources are different facts. Scope observation must not consume a
  child completion. Fixed bounds must leave PID 1 recovery capacity.
- Foreground completion must withdraw readiness immediately, but must not finish
  a service stop/restart while scoped descendants/resources remain. Add a tested
  cleanup acknowledgement to the core/native adapter before enabling scope-wide
  supervision; do not merely hide delayed teardown behind a longer grace timer.
- Owner death must close owned scopes and reject new work without allocation in
  the kill path. Existing live parent ownership wins first, then a live owning
  manager, then PID 1. Filesystem operations that release kernel entry need explicit
  reservation/cancellation and quiescence handling before old resources are reused.
- Keep configured user services separate from transient application launches.
  M6's compositor/session failure policy must close affected application scopes;
  ordinary apps must not acquire restart policies or become persistent units.

## Remaining milestone acceptance

Finish the Ring 3 authenticated scope/session factory, scoped spawn/status/events,
and remaining filesystem allocation/grant, socket and audio accounting/permissions.
Finish temporary-I/O reservations/cancellation and quiescence
checks, home/tmp grants, old-syscall bypass denial and least-authority IPC.
Bind Initator's launches to service scopes and wait for scope cleanup in stop/restart
jobs. Kernel-only identity, frame/thread, IPC and descriptor tests do not complete
these gates.
Only then launch Nest as an explicitly unauthenticated fixed developer session
under a distinct kernel user/session identity, with two supervised user services.
Test independent sessions, failures, cleanup and resource pressure in QEMU at
1/8/32 CPUs; preserve the stricter worker profile and default desktop compatibility.

[The roadmap](init-system-plan.md#m5--enforced-identities-service-scopes-and-a-non-graphical-user-manager)
contains the full planned scope. This milestone must deliver one PR based on
`feat/initator-m4` after its complete checks pass. No login/password verification,
POSIX credential API, physical terminal or graphical session is claimed by the
initial principal types.
