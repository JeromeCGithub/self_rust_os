# Native threads and symmetric multiprocessing

The GRUB QEMU `pc` platform discovers and starts **1–32 enabled logical CPUs**.
The desktop runner defaults to eight. Ring 3 applications can create native
kernel-scheduled threads through `userlib` and share their process's private
memory. This is the OS's native SDK, not a port of Rust `std` or pthreads.

## Running and using threads

```bash
RUSTOS_CPUS=8 cargo run
RUSTOS_CPUS=1 cargo run
RUSTOS_CPUS=32 cargo run
```

The runner validates the CPU count and passes it to QEMU as `-smp`. CPU discovery
uses firmware, rather than assuming the launcher count or contiguous APIC IDs.
Legacy bootimage test kernels retain their one-CPU boot path.

Enable native threading in an application's manifest:

```toml
[dependencies]
userlib = { path = "../userlib", features = ["threads"] }
```

Build `alloc` along with `core`/`compiler_builtins` in its custom target config,
as in [the demonstration application](../user_programs/zthreads/.cargo/config.toml).
Keep the project's pinned toolchain and soft-float target features.

```rust
use userlib::{println, thread};

fn main() {
    let worker = thread::spawn(|| (0u64..100_000).sum::<u64>())
        .expect("worker creation");
    println!("CPUs: {}", thread::available_parallelism());
    println!("result: {}", worker.join().expect("join"));
}
```

`Builder::new().on_cpu(index).stack_size(bytes).spawn(closure)` optionally selects
one online CPU and a guarded stack size. Normal workers can run on any online
CPU. Closures and results require `Send + 'static`; borrowed local captures,
`Rc` captures and mutex guards cannot be transferred to another thread.
`current()` returns a generation-checked TID; `current_cpu()` is an instantaneous
logical CPU index, which may change after a call unless affinity is restricted.

`JoinHandle::join` consumes the handle and transfers the normally returned value
after completion. Dropping a handle detaches without blocking. The runtime keeps
closure/result ownership through an `Arc`; failed kernel creation returns the
unpublished closure allocation to its caller. Kernel rejection is reported as
`thread::Error`; user-heap exhaustion and panics terminate the whole process.
There is no unwind recovery or mutex-poisoning contract.

`userlib::sync` supplies `Arc`, a blocking `Mutex`, `Condvar`, and `Once`. Mutexes
use acquire/release atomic fast paths and park on contention. Wait on condition
variables in a predicate loop. `thread::wait` and `wake` expose process-private
atomic-word parking; always recheck the application predicate after waking.
`thread::yield_now` gives other ready threads a turn, and `sleep_ticks` blocks
only the caller in the existing PIT tick units (approximately 55 ms).

The `/BIN/ZTHREADS.BIN` launcher runs the native demonstration/regression program.
It reports CPU coverage, synchronization and cleanup; its negative tests run in
separate child processes so intentional guard faults do not kill the caller.
Its small temporary configuration files live under `/TMP`, entirely as userspace
test policy. The kernel contains no threading-demo executable or filename policy.

## Kernel architecture and privilege boundaries

| Location | Owns |
| --- | --- |
| [CPU architecture](../src/arch/x86_64/mod.rs) | ACPI topology parsing, AP startup, LAPIC access, CPU identity, GDT/TSS, exceptions, IRQ/syscall entry and TLB invalidation |
| [Process resources](../src/process/mod.rs) | PID identity, shared resources, bounded reusable PCB storage, thread ownership and states |
| [Scheduler](../src/scheduler.rs) | Ready queue, CPU execution ownership, context selection, blocking/wakeup, join/detach and deferred reclamation |
| [Thread lifecycle](../src/scheduler/threads.rs), [reclamation](../src/scheduler/reclaim.rs) | Guarded stacks, join/detach, atomic-word parking and deferred resource release |
| [Owned frames](../src/memory/owned.rs) | Fallible allocation transactions, ownership transfer and rollback |
| [User mappings](../src/userspace/process.rs) | Kernel-side binary, initial-stack and surface mapping mechanisms |
| [Syscall boundary](../src/userspace/syscall.rs) | Argument validation and kernel operation dispatch |
| [User copies](../src/userspace/usercopy.rs) | Effective page-permission checks and assembly byte-copy boundary |
| [Device drivers](../src/drivers/mod.rs) | Hardware register access, transport and device-owned buffers |
| [Filesystem service](../src/fs.rs) | Serialized FAT32/ATA ownership, independent of scheduler entry |
| [Native SDK](../user_programs/userlib/src/thread.rs), [synchronization](../user_programs/userlib/src/sync.rs) | Ring 3 closure execution, join results, mutexes/condition variables and allocation |

All of `src/`, including `src/userspace/`, remains Ring 0. Application closures
execute only in Ring 3. Shell command parsing, window policy, work partitioning
and test-program behavior remain in `user_programs/`. PID 0 retains its existing
display-owner exception; TIDs grant no extra resource privileges.

A process owns code/data, page tables, its initial stack, surface, inbox,
standard-I/O connection and PID-owned sockets. Its threads share these resources
but own separate user/kernel stacks, register frames, wait state and join status.
The CPU-local current TID resolves to the resource-owning PID for existing calls.

Every CPU has its own TSS/GDT, double-fault and NMI stacks, local scheduling timer
and idle stack. Only BSP tables are static before heap initialization; AP tables
live on the supervisor heap, avoiding collision with fixed-address user images.
`src/gdt.rs` and `src/interrupts.rs` preserve existing kernel import paths through
small architecture re-exports.

## Scheduling, concurrency and memory

A shared round-robin queue initially schedules all native threads. Optional
affinity is checked before selection. The fair architecture entry gate protects
scheduler and mapping transitions across the actual assembly stack handoff;
another CPU cannot restore an outgoing context before its old stack is inactive.
Waiting CPUs halt with IRQs enabled and targeted IPIs wake the next gate owner.
Device IRQs perform bounded transport work without taking the entry gate;
ordinary scheduler entry consumes pending keyboard wakeups. Kernel-mode IRQs do
not preempt arbitrary kernel lock holders. They record a CPU-local reschedule
request, which syscall return services after dropping service locks. User instructions run simultaneously
on available CPUs, and excess runnable threads share time through preemption.
When no alternative is ready the current context continues. Switching between
threads sharing a root preserves the TLB; mapping changes still explicitly flush
all affected CPUs before reuse.

Mailbox waiting blocks the calling thread until its PID-owned queue has data or
its deadline expires. Queue inspection and parking share scheduler serialization
with send, preventing lost notifications. This is a readiness hint; another
same-process reader can consume the message first. The window SDK and compositor
choose this primitive for protocol waits, with a one-tick deadline for periodic
work; the kernel does not interpret frame or widget messages.

Filesystem calls first copy paths/data into owned kernel buffers, release CPU
entry, and acquire the filesystem's service lock. They release that lock before
reacquiring entry. This lets other CPUs schedule, render and service syscalls
while one CPU performs PIO. Local IRQs remain enabled during service work, so PIT
ticks and input still progress when the BSP performs I/O. CPU execution ownership retains an in-flight syscall's
process/stack; a concurrent kill cannot free them underneath it. Returning from
the service checks stopped-thread state before resuming user mode or publishing
a newly loaded process. Other core services currently retain coarse serialization.

The BSP's PIT remains the public timebase. Local APIC timers are calibrated with
PIT channel 2 and drive local scheduling without multiplying public ticks by the
CPU count. Device interrupts retain PIC virtual-wire delivery on the BSP. This
implementation does not provide I/O APIC routing or x2APIC mode.

Idle secondary CPUs mask their local scheduling timers and wake through targeted
work IPIs. The BSP continues servicing time. If a PIT tick interrupts BSP kernel
work, it requests deadline servicing on an idle AP without taking scheduler locks;
resuming a user thread unmasks that AP's calibrated preemption timer.

Complete buffers are checked through every page-table level for present/user and
effective write permissions. Assembly byte copies avoid Rust references into RAM
that siblings can modify. Syscall records are validated after copying into owned
initialized kernel storage. Mapping mutation remains under entry serialization;
no user reference survives copying, blocking or the filesystem service boundary.

Worker-stack teardown removes mappings and cancels waiters for reused addresses.
Lock-independent NMI handlers reload CR3 and acknowledge an invalidation generation
on every online CPU before frames are recycled. They use CPU-private IST stacks
and acquire no scheduler, allocator, filesystem or entry locks. CPUs cannot switch
address spaces while an invalidation owner holds entry. PCID/global mappings are
not enabled. Reclamation never frees the currently executing kernel stack.

Process exit/kill stops all siblings; `waitpid` reaps the PCB and completion code.
Frames and stacks remain owned until remote execution, in-flight services and TLB
references are quiescent. Exited joinable threads retain bounded completion/stack
ownership until joined or detached. Failed unpublished process construction rolls
back every owned frame. Reaped PCB storage is reused while PID values continue
monotonically, preventing kernel-heap growth from empty PID-indexed history.

Normal `main` return exits the whole process. Native closure return exits only
that thread; last-thread exit completes process teardown. User page faults and
general-protection faults terminate that process with status 14 or 13 respectively;
supervisor faults remain fatal. Arbitrary forced cancellation of individual
threads is not exposed.

## Syscalls

All use existing `int 0x80` registers (`rax` number; `rdi/rsi/rdx` arguments).
Unspecified/reserved arguments must be zero. General rejection is the existing
`SYSCALL_ERROR`; internal exit/block sentinels are consumed by architecture entry.

| Number | Operation | Arguments and result |
| --- | --- | --- |
| 96 | CPU count | No arguments; online count, 1–32 |
| 97 | Current CPU | No arguments; dense logical CPU index |
| 98 | Create thread | Request pointer, exact size 40, zero; returns generation-checked TID |
| 99 | Thread exit | No arguments; terminates only caller, never returns |
| 100 | Join | Same-process TID, zero, zero; zero on normal completion; may block |
| 101 | Detach | Same-process TID, zero, zero; zero on success |
| 102 | Yield | No arguments; zero after resuming |
| 103 | Current TID | No arguments; generation-checked caller identity |
| 104 | Compare/wait | Aligned atomic-word address, expected `u32`, timeout ticks (zero indefinitely); 0 wake, 1 mismatch, 2 timeout |
| 105 | Wake | Atomic-word address, maximum waiter count (0–1024), zero; count actually awakened |
| 106 | Stack diagnostics | No arguments; current process's allocated worker-stack pages |
| 107 | Frame diagnostics | No arguments; scheduler allocator's currently owned physical-frame count |
| 108 | Mailbox readiness | Timeout ticks (zero indefinitely), zero, zero; 1 when the process mailbox is nonempty, 0 on timeout; does not consume a message |

The [padding-free create record](../crates/thread-abi/src/lib.rs) contains version
`u32=1`, reserved flags `u32=0`, user entry `u64`, opaque user argument `u64`, usable
stack bytes `u64`, and allowed CPU mask `u64` (zero selects all online CPUs).
The kernel allocates/maps the stack and validates entry mappings. Existing disk
binary permissions still lack complete W^X enforcement; this ABI does not claim
an executable-only code region.

Join rejects self, stale/foreign handles, detached threads and competing joiners.
Exactly one join consumes completion. Atomic wait keys include PID and user
address; equal virtual addresses in different processes are separate. The final
expected-value check and queue registration are serialized with wakeup, preventing
the wake-before-park race. No user pointer is dereferenced while the waiter sleeps.

## Limits

- QEMU BIOS/GRUB `pc`, xAPIC, 1–32 enabled logical CPUs with APIC IDs below 256.
  Firmware tables must fit the boot identity mapping's first GiB. Unsupported or
  malformed topology and failed AP startup stop boot explicitly. Physical hardware,
  EFI discovery, CPU hotplug, NUMA and x2APIC are not verified/supported here.
- At most 128 live/unreclaimed processes, 1024 thread records globally and 64
  threads per process, including its initial thread. Live and unreaped completion
  records consume quotas. Creating a thread can fail; join or detach handles.
- Initial user stacks remain 4 MiB. Workers request 16–256 KiB, page-rounded,
  plus an unmapped 4 KiB guard. Their 63 fixed reservations start at `0x4000000`
  with `0x41000` spacing inside the private region ending at `0x6000000`.
  Existing binaries/stacks/surfaces keep their addresses; surfaces stop at
  `0x4000000`. Kernel thread stacks are 16 KiB; CPU idle stacks are 64 KiB.
- The `threads` feature enables a reclaimed, mutex-protected 1 MiB process heap.
  Combining it with the existing HTTPS `tls` feature retains that feature's
  16 MiB heap. Heap exhaustion and panics abort the process.
- Targets retain software floating point and disabled MMX/SSE. Hardware FPU/SIMD
  context switching, compiler-generated thread-local storage/destructors, scoped
  threads, a general kernel-worker API and a Rust `std` port remain future work.
- Kernel entry and network/device services retain coarse locks; CPU-intensive
  application work can scale while serialized services remain throughput limits.
  Applications must partition work explicitly; eight CPUs do not automatically
  accelerate a single sequential thread.

## Validation

Build fresh images before runtime checks; every test boots a private writable
copy. Preserve timeout values and exact counter/error/cleanup assertions.

```bash
./tools/build-grub-disk.sh target/smp-current.img
RUSTOS_TEST_DISK=target/smp-current.img python3 -B e2e/smp.py --cpus 1 2 8 32
RUSTOS_TEST_DISK=target/smp-current.img python3 -B e2e/smp.py --cpus 8 --stress
RUSTOS_TEST_DISK=target/smp-current.img RUSTOS_CPUS=8 python3 -B e2e/desktop.py
python3 -B tools/thread_api_check.py
rustc --edition=2021 --test src/arch/x86_64/acpi.rs -o /tmp/rustos-acpi-tests
/tmp/rustos-acpi-tests
rustc --edition=2021 --test crates/thread-abi/src/lib.rs -o /tmp/rustos-thread-abi-tests
/tmp/rustos-thread-abi-tests
cargo test --test threads --test display_mapping --test stdio --test keyboard_timer
```

The Ring 3 probe pins one worker to each CPU, requires a shared-memory rendezvous
with distinct CPU identities, and checks computed results/private stacks. It tests
mutex contention, condition variables, Once, compare/wake/timeout, stale/self
handles, invalid records and guard mappings, repeated join/detach frame recovery,
independent process cleanup, isolated guard faults, and a cached remote stack
access after unmapping. It also kills a process with a filesystem worker and
checks deferred reclamation. `--stress` exhausts physical memory, checks failed
process-construction rollback and recovers all committed children.

The kernel ownership harness additionally checks foreign-thread rejection,
process-private wait keys, allocation-transaction rollback, physical-frame
recovery and constant kernel-heap usage across repeated process creation. SDK
compile checks include a valid owned capture and rejected `Rc`, borrowed-stack
and mutex-guard captures. Parser tests cover all counts 1–32 and malformed/sparse
topology. Runtime logs go under `target/smp/`; these are generated evidence, not
tracked images. Run the remaining union of [subsystem checks](testing.md), including
network, storage, HTTP/TLS and GUI checks when their integration paths change.

QEMU execution is evidence for the tested virtual platform. QMP screenshots and
injected input do not establish manual host GUI behavior or physical-hardware
multicore operation; those checks must be reported separately.
