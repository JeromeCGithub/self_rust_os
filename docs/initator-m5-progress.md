# Initator M5 work in progress

M4 is [PR #57](https://github.com/JeromeCGithub/self_rust_os/pull/57), open and
unmerged, with passing documentation CI at `7f8b750`. Its local runtime evidence
is in [the M4 record](initator-m4-progress.md). M5 starts from that commit on
`feat/initator-m5`; no M5 PR exists yet.

**M5 is incomplete.** The first addition is a host-tested principal description and
attenuation policy in [process-abi](../crates/process-abi/src/principal.rs). It does
not yet attach credentials to kernel processes, enforce these capability bits,
create scopes or run Nest. The syscall constant reserves 129; its dispatcher, SDK,
native callers and authority/resource enforcement remain to be implemented in this
same milestone before delivery. Do not interpret a decoded description as a grant.

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

- [Process metadata](../src/process/mod.rs) has no credentials/scope/capability
  fields. It contains parent/adoption ownership, private frame/surface ledgers,
  descriptors and an optional restricted-worker heap ceiling. Threads share their
  process resources. The process table has 128 reusable storage slots but externally
  monotonic, unreused PIDs; scope membership must not use a PID as a bit index.
- [Common construction](../src/scheduler.rs) is the single Process initializer;
  every old/new spawn path must inherit or explicitly attenuate its principal
  there before grant/frame publication. Current construction initializes the
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

`target/initator-m5-evidence/principal-red.log` records three expected failures:
missing codec, inherited init authority and unavailable valid attenuation.
`principal-green.log` records those three new tests and the existing process-ABI
suite passing after implementation. The tests cover exact wire shape and reserved
bytes, short/trailing data, inconsistent system/user/legacy contexts, invalid IDs,
ordinary-child identity/scope preservation, non-inheritable authority and attempted
rights expansion. These are host policy/codec results, not native enforcement.

## Remaining milestone acceptance

Implement/test kernel-owned credentials and query, atomic scoped/delegated spawn,
bounded scopes and all selected resource accounting, owner-death/descendant cleanup,
alias-safe filesystem grants, old-syscall bypass denial and least-authority IPC.
Only then launch Nest as an explicitly unauthenticated fixed developer session
under a distinct kernel user/session identity, with two supervised user services.
Test independent sessions, failures, cleanup and resource pressure in QEMU at
1/8/32 CPUs; preserve the stricter worker profile and default desktop compatibility.

[The roadmap](init-system-plan.md#m5--enforced-identities-service-scopes-and-a-non-graphical-user-manager)
contains the full planned scope. This milestone must deliver one PR based on
`feat/initator-m4` after its complete checks pass. No login/password verification,
POSIX credential API, physical terminal or graphical session is claimed by the
initial principal types.
