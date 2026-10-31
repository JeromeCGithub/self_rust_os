# Native threads and multicore execution plan

Historical design baseline, based on source reviewed at `beddaad` on `main`.
The following proposal records the original single-CPU assumptions and intended
stages. The [current threading guide](threading.md) describes what was actually
implemented, including the entry gate, filesystem lock, PIC virtual wire and
remaining limitations. This proposal is not the current implementation contract.

The goal is to let independent applications and multiple threads within an
application execute simultaneously on available x86-64 logical CPUs. Applications
should create, join and synchronize threads through safe Ring 3 `userlib` APIs.
Scheduling must still work with one CPU and with more runnable threads than CPUs.
Logical CPUs include hardware threads on a core; their count is not a promise of
equivalent performance to that many physical cores.

## What exists and what needs to change

| Area | Source evidence today | Required change |
| --- | --- | --- |
| Scheduling | [Scheduler](../src/scheduler.rs): one global `current`, PID ready queue, one context and kernel stack per process | Separate process resources from schedulable threads; current thread and idle context per CPU |
| CPU entry state | [GDT/TSS](../src/gdt.rs): one TSS with mutable RSP0 and one double-fault stack | CPU-owned TSS/GDT and exception stacks with valid interior mutability and exclusive ownership |
| Interrupts | [Interrupts](../src/interrupts.rs): PIC IRQs, PIT ticks, preemption on return from user mode | Local APIC support, per-CPU timer/preemption state, IPIs and explicit device IRQ routing |
| Discovery/boot | [Multiboot parser](../src/boot/multiboot2.rs) extracts RAM/framebuffer; [GRUB entry](../src/grub_boot.rs) initializes one CPU | ACPI CPU discovery, AP startup trampoline and bounded online handshake |
| Process memory | [Loader](../src/userspace/process.rs) maps one fixed user stack; scheduler forgets kernel-stack ownership | Separate guarded stacks for each thread, shared owned address space, allocation rollback and deferred reclamation |
| Physical allocation | [Frame allocator](../src/memory.rs) advances through RAM with no general frame-return API | Track ownership and recycle freed frames without double-free or reuse while remotely accessible |
| User buffers | [Older syscalls](../src/userspace/syscall.rs) form user slices; [mapping validator](../src/userspace/usercopy.rs) explicitly relies on single-CPU execution | Concurrent-safe copy boundary, stable mappings and no Rust references into hostile shared user memory |
| User runtime | [Runtime](../user_programs/userlib/src/rt.rs) starts one `main`; [heap](../user_programs/userlib/src/heap.rs) is enabled for the TLS networking feature | Thread trampoline, general allocation feature, join state and explicit panic policy |
| Execution tests | [QEMU profile](../tools/qemu_platform.py) has no `-smp` option | Explicit 1/2/4/8-CPU profiles and actual Ring 3 parallelism, isolation and lifetime tests |

The cooperative executor under `src/task/` does not supply native threads.
Adding `-smp 4` makes virtual CPUs available but does not start them or make kernel
state safe for concurrent access.

## Responsibility and execution model

Use one kernel-scheduled thread per application thread, initially with a single
shared ready queue. Kernel threads can use the same scheduling core later, with
their own entry and context rules. A small global queue makes correctness easier
to establish; per-CPU queues and work stealing are subsequent optimizations.

```text
Ring 3: application
        userlib::thread / sync / channels / worker pool
                      |
             typed syscall ABI
                      |
Ring 0: process owns address space and shared resources
        thread owns saved context and execution stacks
                      |
              shared ready queue
             /        |        \
          CPU 0     CPU 1     CPU 2 ...
        current     current   current
        TSS/timer   TSS/timer TSS/timer
        idle stack  idle stack idle stack
```

Keep PID-based privileges and resource ownership on the **process**. All its
threads share its private address space, surface, inbox, standard-I/O endpoint
and PID-owned sockets. A TID identifies an execution context and grants no extra
rights. PID 0's existing compositor privilege remains a process-level exception.
No kernel pointers or arbitrary hardware controls cross the ABI.

The new internal ownership split should be approximately:

| Object | Owns |
| --- | --- |
| `Process` | PID, shared address-space owner, process lifecycle, resource locks, thread membership and quotas |
| `Thread` | Generation-checked TID, process reference, run/block state, CPU ownership, saved registers, user/kernel stack owners, join status and thread-local state |
| `CpuLocal` | Logical CPU/APIC identity, current TID, idle stack, TSS/GDT, exception stacks, scheduling quantum and reschedule flag |
| `AddressSpace` | Page tables, owned user frames, mapping lock, CPUs using the address space and TLB invalidation generations |

`current_pid()` resolves through the calling CPU's current thread. Switching
between threads of one process can keep CR3 when mapping-generation rules allow
it. Switching processes installs the new address space. Every switch updates the
local TSS RSP0 to the incoming thread's private kernel stack.

## Correctness work required before multicore applications

### Scheduler and wait handoff

A thread is running on at most one CPU. Queue membership, state, saved context
and CPU ownership form one transition protected by a defined protocol.
The current single-CPU pattern of marking a process blocked before saving its
context cannot simply be reused: another CPU could wake it before its context
is ready. Similarly, publishing a preempted thread to the queue before leaving
its kernel stack lets another CPU resume that same stack too early.

Introduce an explicit switch handoff, such as CPU ownership plus a pending-wakeup
state. Save the context and finish executing on the old stack before making the
thread eligible elsewhere. Preserve wakeups that arrive during this handoff.
Do not hold a Rust mutex guard across an undocumented assembly stack switch.
Each CPU idles on its own stack; a dead thread's stack cannot double as idle
storage. Cross-CPU wakeup publishes runnable state before sending a reschedule
IPI, and coordinates with the target's idle check to prevent lost wakeups.

Initially preserve the existing user-mode preemption rule. Kernel execution may
run concurrently on different CPUs, so this rule provides no shared-state lock.
Track critical sections and bounded kernel work; kernel-thread support must have
explicit yield/block points. General kernel preemption is later work, not a
reason to disable user preemption permanently.

### Interrupts, locking and shared services

Existing `spin::Mutex` locks provide mutual exclusion across CPUs, but do not by
themselves handle interruption of a lock holder, sleeping while locked or lock
order. Disabling interrupts affects only the local CPU. Use short IRQ-safe
spinlock sections, explicit acquire/release publication and documented lock order.
Do not allocate, perform disk operations or render surfaces while holding the
run-queue lock. Move cleanup outside it with stable ownership references.

Audit the allocator, scheduler, filesystem, ATA, PCI configuration ports, CMOS,
network/NIC, input queues, framebuffer, serial logging and every mutable global
before exposing shared execution. The network facade already documents scheduler
to network lock ordering; preserve or deliberately replace it everywhere.
Device locks can initially serialize operations. IRQ handlers enqueue bounded
work and acknowledge the right controller; worker threads can perform deferred
work after kernel-thread execution is supported.

Local APIC timers drive local scheduling. Keep a separate monotonic time source
for deadlines. The existing PIT-based `ticks()` and `sleep_ticks()` contracts
must not run N times faster on N CPUs; changing tick units needs an explicit
migration, including network time conversion and game timing.

### User memory and TLBs

Two issues need separate solutions:

1. **Mapping lifetime:** serialize page-table changes, hold an address-space
   lifetime reference and pin or lock relevant mappings during copies. Handle
   invalid mappings and copy faults without killing the kernel. Never retain
   user references while blocked or across scheduling/unmapping.
2. **Concurrent contents:** a sibling can change bytes while the kernel reads.
   Copy syscall records into owned initialized kernel storage with a user-access
   primitive designed for concurrent access; validate the copied record. Plain
   Rust slices and ordinary raw copies are not a complete hostile-memory design.
   Recheck lengths/indices from the local copy, bound work, and define partial
   output behavior. A mapping lock does not lock application writes.

Inventory both the older range-only paths and the newer network/platform copies;
their single-CPU safety arguments must be replaced. Overlapping simultaneous
output calls and malformed concurrently modified records must not produce
kernel aliasing violations or invalid typed values. Use an architecture-specific
copy boundary with a documented compiler/assembly contract and fault recovery.

Unmapping or restricting a shared address space requires TLB shootdown: publish
the change, invalidate locally and on every relevant CPU, and wait for confirmed
completion before recycling frames. Coordinate CPU entry/exit with mapping
generations so migration cannot miss an invalidation. Shootdown handlers must
acknowledge without acquiring locks held by the sender; waiting for a remote CPU
that has disabled interrupts while spinning on such a lock can deadlock too.
Shared kernel mappings require corresponding invalidation coverage.

### Stacks, lifecycle and CPU context

Allocate distinct guarded user stacks and owned kernel stacks. The current
private region ends at `0x4000000`, and binaries/BSS, the 4 MiB initial stack
and the display surface already consume substantial space. Measure actual
images and choose a bounded stack arena and thread quota. Do not assume gaps are
free. Preserve initial addresses where feasible; if more virtual space is needed,
migrate validation, loaders, SDK constants and tests together without silently
overlapping existing mappings. Define initial stack alignment and entry registers.

Add frame return, page-table teardown, partial-spawn rollback and a deferred
reaper. The reaper may free a stack only after execution has switched away, and
free user mappings only after copies, remote execution and TLB references are
finished. Joinable thread completion retains bounded join metadata; detaching
must eventually release it. Limit unreaped completions as well as live threads.
Thread quotas must account for mapped stack bytes and kernel memory, not just
thread count. Repeated create/join and create/detach must return resource counts
to baseline after deferred work completes.

Keep existing process `exit` semantics: it stops all threads. A new
`thread_exit` stops only the caller; last-thread exit completes process teardown.
Define normal `main` return as process exit, matching existing applications.
Process kill first prevents new spawns, requests remote stops, cancels waits and
waits for safe execution quiescence before shared-resource destruction.
Never asynchronously free resources under a sibling running in a syscall.
Initially omit arbitrary per-thread forced cancellation; cooperative cancellation
is much easier to make safe for user-held locks.

The current saved context includes general registers and the interrupt frame.
Define per-thread FS base for a future user thread-local block, with separate
kernel CPU-local storage and validated addresses. Save/restore the relevant bases
on migration. The current targets disable MMX/SSE and use software floating point;
keep those restrictions initially. Before enabling hardware floating point or
SIMD, initialize CPU features consistently on every CPU and preserve/test x87,
SIMD and extended state using a supported FXSAVE/XSAVE strategy.

## Bringing additional CPUs online

Implement GRUB SMP first; keep the legacy bootimage path explicitly single-CPU
until its discovery/startup path has its own tests.

1. Extend Multiboot2 parsing to preserve ACPI RSDP information (old/new ACPI tags).
   Parse RSDT/XSDT and MADT with checksum, length, overflow and physical-mapping
   validation. Collect enabled CPU APIC IDs, local APIC address overrides, I/O
   APICs and interrupt-source overrides. Firmware addresses must actually be
   accessible; a direct-map offset alone does not prove that.
2. Start with a declared xAPIC platform subset. Detect x2APIC requirements and
   unsupported IDs/topologies explicitly; do not truncate IDs or claim universal
   PC support. Verify features on each CPU. No CPU hotplug or NUMA policy is
   required for the first release.
3. Reserve a firmware-safe, page-aligned startup trampoline below 1 MiB, along
   with transition page tables and per-AP boot stacks. Arrange CR3 and mode
   transitions for the constraints of the startup code. Do not reuse the BSP's
   boot stack or overwrite firmware/boot metadata.
4. Enable/map local APIC registers with correct memory attributes. Send the
   architecture-prescribed INIT/STARTUP sequence with bounded waits. Each AP
   enters long mode, installs its own stack, CPU-local state, GDT/TSS and IDT,
   then publishes readiness with release/acquire synchronization.
5. Keep APs parked until shared services and scheduling are safe. Bound failed
   startup, exclude failed CPUs from scheduling, and prevent a late AP from
   accessing reused startup resources. Define fallback behavior for missing ACPI
   or unsupported SMP hardware; one-CPU operation must remain usable.
6. Add local timers, reschedule and TLB IPIs. Route initial device interrupts to
   the BSP through an explicitly tested I/O APIC setup, honoring source overrides
   and trigger/polarity. Mask the old PIC appropriately and prevent duplicate
   delivery. Calibrate timing rather than assuming an APIC timer rate.

Follow the [Intel system programming manuals](https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html)
for AP startup, APIC delivery and translation invalidation, and the
[ACPI specification](https://uefi.org/sites/default/files/resources/ACPI_Spec_6.6.pdf)
for firmware table formats. These are implementation references, not evidence
that this repository implements the mechanisms.

## Syscall and userspace library proposal

Keep `int 0x80` and existing syscall numbers. Introduce a versioned shared ABI
crate, following the existing network/platform ABI pattern, and allocate numbers
only during implementation after checking the entire dispatcher. A create record
fits the current three-argument convention through a pointer and explicit size.

| Proposed operation | Contract to implement and test |
| --- | --- |
| Thread create | Copy a versioned, fixed-width request containing user entry, argument and stack size; kernel allocates/maps stack and returns a generation-checked handle |
| Thread exit | Finish only the caller, publish completion, schedule away; no return |
| Join | Same-process handle, exactly one successful join; block without retaining user buffers; reject self/stale/foreign handles and competing joins |
| Detach | Release join ownership once; completion triggers eventual reclamation |
| Current TID / yield | Identify caller; yield lets another ready thread run, including on one CPU |
| Wait / wake | Process-private aligned atomic-word wait keyed by address-space identity and address, with an atomic compare-and-enqueue protocol |
| CPU information | Report supported/online logical CPU counts and bounded logical identity data for diagnostics and worker-pool sizing |

Reject unknown versions/flags, nonzero reserved fields, overflowing sizes,
unmapped/non-user/non-executable entries as enforced by actual mapping flags,
invalid stack sizes, quota exhaustion and allocation failures with typed errors.
Creating a thread must either succeed completely or leave no handle, frames or
queue entry behind. Existing incomplete W^X enforcement must remain documented.

For wait/wake, define mismatch, timeout and spurious-wakeup results. Coordinate
the final expected-value check with queue registration so wakeup cannot be lost.
Do not store a Rust reference to the user's atomic word while asleep. Coordinate
unmapping/address reuse with waiter cancellation and process exit. Build mutex
fast paths with acquire/release atomics in user space, parking only on contention;
condition variables release the mutex and enter the wait protocol safely.

Expose `userlib::thread::{spawn, Builder, JoinHandle, current, yield_now}` and
`userlib::sync::{Mutex, Condvar, Once}`; re-export `alloc::sync::Arc` once general
allocation is available. Channels and a bounded worker pool can follow.
`spawn` should return `Result<JoinHandle<T>, SpawnError>` to expose exhaustion.
Both the closure and return value require `Send + 'static`, following the safety
constraints of [Rust's native thread API](https://doc.rust-lang.org/std/thread/fn.spawn.html).
`Mutex<T>` should implement `Send`/`Sync` only with the appropriate bounds; a guard
should not migrate to a different thread if unlocking requires owner identity.

Illustrative application API, **not available today**:

```rust
use userlib::{println, thread};

fn main() {
    let a = thread::spawn(|| (0u64..100_000).sum::<u64>())
        .expect("first worker");
    let b = thread::spawn(|| (100_000u64..200_000).sum::<u64>())
        .expect("second worker");
    println!("sum = {}", a.join().expect("join a") + b.join().expect("join b"));
}
```

The runtime boxes the closure and join result in process-owned storage. A generic
Ring 3 trampoline executes the closure and calls thread exit; the kernel never
executes Rust closures. Transfer ownership exactly once, including when a child
runs before create returns, creation fails, a handle is dropped or a thread is
detached. Join establishes visibility of the completed thread's writes. Dropping
a handle detaches rather than implicitly blocking in a destructor.

Make allocation a general feature instead of tying it to the `tls` networking
feature. Audit SDK auto traits, window singleton ownership, console serialization,
socket close/use races and shared protocol inbox consumption. Keep UI event
handling/drawing on one designated thread per process initially, with message
passing to workers. A surface revision or serial alone does not synchronize
concurrent pixel writes and snapshots; strengthen the cross-process publication
protocol for SMP as well. Existing single-CPU safety comments need re-evaluation.

Keep `panic=abort` for the first release and terminate the process on any thread
panic. No unwind-based panic recovery or poisoned-mutex promise is implied.
Normal closure return can produce a joined value; process-fatal panic cannot
promise a recoverable panic payload. Compiler-generated thread-local storage
and TLS destructors require additional loader/linker/runtime work; a manually
managed per-thread runtime block is sufficient initially. Here thread-local
storage is distinct from the existing HTTPS/TLS feature.

This makes `userlib` the OS's native standard SDK. Actual `std::thread` support
is a separate Rust standard-library port for this target: thread backend,
parking/synchronization, TLS, allocation, time and the other platform facilities
needed by that port. It is not obtained by renaming a crate or enabling `alloc`.
Do not change the pinned compiler/target as an incidental threading change.

## Delivery sequence and red-to-green acceptance

Each row is a series of small reviewable changes. Add the behavioral test first,
observe the expected missing-behavior failure, implement it, then run the union
of applicable suites from the [testing matrix](testing.md#change-to-test-matrix).
The tests below are proposed additions, not commands already available or passed.

| Stage | Deliverable | Required evidence before proceeding |
| --- | --- | --- |
| 1 | Separate process/thread ownership, retain one initial thread and current ABI | Existing Ring 3 process progress, blocking/wakeup, input, exit/kill/wait, desktop and networking remain correct with one CPU; state-transition tests prohibit duplicate execution |
| 2 | Owned reclaimable frames/stacks, concurrent-safe user-copy boundary, guarded thread-stack arena | Injected allocation failures roll back fully; invalid/cross-page/read-only buffers rejected; teardown cannot free an executing stack; repeated lifecycles restore resource counts |
| 3 | Single-CPU thread syscalls, trampoline, allocation and join/detach | Two actual Ring 3 threads share data but have distinct stacks/TIDs; return/join visibility; self/foreign/stale/double join rejected; main return, last-thread exit and process kill obey lifecycle contracts |
| 4 | Wait/wake plus safe SDK mutex/condition variable | Exact shared counter under contention, forced wake-before-park race, timeout/spurious wake, address reuse, exit cancellation; compile-fail cases reject non-Send closures and borrowed stack captures |
| 5 | ACPI/APIC and AP startup; APs remain parked | Malformed table tests, enabled/disabled/duplicate/noncontiguous IDs, bounded missing-AP case, distinct CPU stacks/TSS, online counts with 1/2/4/8 QEMU CPUs; device routing remains correct |
| 6 | Complete shared-service audit, safe switch handoff, remote wake/stop and TLB shootdown; enable AP scheduling | Parallel Ring 3 execution on distinct CPUs, one execution owner per TID, cross-CPU wait/kill races, stale-translation rejection after unmap/remap and frame reuse, multi-process isolation |
| 7 | Worker pool, diagnostic/demo applications and kernel-worker context support | User launches a thread demo; reports online CPU count and verified result; desktop/input/network remain usable under load; deferred device work observes bounded IRQ behavior |
| 8 | Profile and improve scaling | Equivalent fixed workloads at 1/2/4/8 CPUs, exact results, elapsed time/throughput and per-CPU usage; use measurements to justify per-CPU queues, finer locks or buffer partitioning |

Stages 3-4 and parked-AP bring-up can be developed independently after their
prerequisites, but shared-state safety must precede scheduling applications on
APs. QEMU host tooling changes need executable host regressions first; extend the
shared profile rather than adding unrelated hard-coded launchers.

For a strong parallelism test, pin two probe threads to different CPUs through
a test-only mechanism, publish their per-CPU execution markers and require a
bounded rendezvous while neither yields. Assert distinct logical CPU IDs and
both completed results. Two serial messages or faster elapsed time alone do not
prove simultaneous execution. Keep timing benchmarks separate from correctness
checks; QEMU TCG timing is not a physical-core speedup guarantee.

Run scheduler/lifecycle, memory, syscall/SDK, boot and changed driver suites under
the supported CPU profiles, including one CPU. Include fresh GRUB desktop E2E,
network/HTTP/TLS and storage integration after shared-service changes. Preserve
the existing timeout and baseline-failure reporting rules. Fault injection should
force remote wakeup while context save is pending, kill during a syscall and
shootdown while another CPU enters/leaves an address space. Bounded user loops
must leave the kernel usable after failed probes.

## Applying concurrency throughout the OS

Programs can distribute CPU work once threading exists; sequential code does
not automatically become parallel. Start with independent application threads
and a worker count derived from available CPUs, then measure these opportunities:

| Subsystem | Initial parallel work | Ownership to preserve |
| --- | --- | --- |
| Applications | Compute, parsing and network preparation in a bounded worker pool | Explicit shared data through typed synchronization/channels |
| Desktop compositor | Independent tile composition into disjoint owned buffers | One coordinator owns stacking, input, publication and final presentation |
| Networking | Deferred protocol progress and application request processing | Serialize each socket/state machine and NIC ring ownership initially |
| Storage | Request preparation and asynchronous callers | One serialized ATA PIO device path initially; simultaneous threads do not create simultaneous disk transfers |
| Kernel services | Deferred IRQ work and resource reaping | IRQ-safe queues, explicit worker lifetimes and no scheduler lock across I/O |

Do not create one thread per pixel, packet or short operation. Use bounded queues,
backpressure, disjoint buffers and cancellation that preserves owners' cleanup.
Measure contention before replacing the global scheduler or allocator locks.

The first useful milestone is a safe single-CPU `spawn`/`join` demo with complete
cleanup. The first multicore milestone is that same shared-memory application
verified on two CPUs, alongside an unaffected second process and usable desktop.
Full `std`, POSIX/pthreads compatibility, CPU hotplug, NUMA scheduling, lock-free
drivers and parallelizing every application are separate follow-up projects.

## Validation of this proposal

This is documentation-only work. Before editing, the documentation checker and
all 24 host tooling tests passed. No failing runtime test was added because no
runtime behavior changed. Run those checks again and inspect `git diff --check`
for this document; record the actual results in the delivery report.

No SMP runtime, resource-reclamation, thread API or performance result is claimed.
A screenshot cannot establish correctness of this design. Future implementation
reports must include observed red failures, actual green checks, serial/runtime
evidence, and remaining manual checks. In particular, QEMU screenshots/input
injection do not replace host GUI testing or physical multicore hardware tests.
