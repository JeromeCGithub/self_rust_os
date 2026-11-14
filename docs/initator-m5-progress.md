# Initator M5 work in progress

M4 is [PR #57](https://github.com/JeromeCGithub/self_rust_os/pull/57), open and
unmerged, with passing documentation CI at `7f8b750`. Its local runtime evidence
is in [the M4 record](initator-m4-progress.md). M5 starts from that commit on
`feat/initator-m5`. M5 is now split into stacked review phases. Phase 1 covers
principal descriptions, kernel-derived inheritance, capability attenuation and
protected system-file mutations. Phase 2 adds the scope model and transaction
primitives; later phases integrate kernel scope enforcement, grants and Nest,
then complete isolation, resource accounting and milestone-wide validation.
Phase 1 is a foundation, not an untrusted multi-user security boundary.

**M5 is incomplete.** The first addition is a host-tested principal description and
attenuation policy in [process-abi](../crates/process-abi/src/principal.rs). The
kernel now attaches native principals, enforces mechanism capability domains and
supports one-way process rights reduction and protects system-file mutations.
It does not yet create resource scopes, enforce session home/tmp grants or run
Nest. Syscalls 129–130 and their SDK wrappers
are wired; the remaining authority/resource enforcement must be implemented in
this same milestone before delivery. Do not interpret a decoded description as a
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
  before grant/frame publication. Explicit scoped grants remain future work. Current construction initializes the
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
  only a restricted worker's heap ceiling; initial image, stack, surface, page
  tables and thread resources require scope accounting too. Charges and physical
  reclamation have distinct lifetimes.
- The default desktop is still the trusted legacy profile. Native M5 session
  enforcement must not be described as protection already supplied to that path.
  Non-PID-0 graphical authority/discovery and compositor lifecycle remain M6.

## Tests run so far

Phase 1 was revalidated in its own worktree: the process ABI host tests, kernel
unit tests, userspace, process-supervision, thread, IPC and FAT32 integration
tests passed, as did the default and GRUB builds. Native principal, capabilities
and protected-system-file probes passed at 1, 8 and 32 configured CPUs. The
single-CPU Initator integration checkpoint also passed two-service supervision,
process lifecycle and exhaustion, IPC lifecycle, cooperative stop, halt ABI and
fatal-init cases. These checks do not establish session or resource isolation.
The eight-CPU legacy desktop QEMU regression passed; its [captured desktop](screenshots/initator-m5-phase1-desktop.png)
shows the tested compatibility boot. No manual host GUI check was performed.

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

These identities currently describe the system context: user 0/session 0/reserved
system scope 1. Actual user/session creation, scope objects/accounting and
per-session filesystem
grants are not implemented yet. Mechanism-domain filtering is now implemented,
but a capability description is not proof that the remaining resource/path
boundaries exist.

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

Finish authorized user/session credential creation and atomic scoped/delegated spawn,
bounded scopes and all selected resource accounting, owner-death/descendant cleanup,
per-session filesystem grants beyond the protected system roots, remaining
old-syscall/session-boundary bypass denial and least-authority IPC.
Only then launch Nest as an explicitly unauthenticated fixed developer session
under a distinct kernel user/session identity, with two supervised user services.
Test independent sessions, failures, cleanup and resource pressure in QEMU at
1/8/32 CPUs; preserve the stricter worker profile and default desktop compatibility.

[The roadmap](init-system-plan.md#m5--enforced-identities-service-scopes-and-a-non-graphical-user-manager)
contains the full planned scope. Phase 1 is based on `feat/initator-m4`; later
phase PRs will be stacked on their predecessors. No login/password verification,
POSIX credential API, physical terminal or graphical session is claimed by the
initial principal types.
