# Current syscall ABI

This is a project-specific interface, **not the Linux syscall ABI**. Compare the
[kernel dispatcher](../src/userspace/syscall.rs) with
[typed SDK wrappers](../user_programs/userlib/src/sys.rs) before changing it.

## Calling convention

Ring 3 invokes `int 0x80`. `rax` contains the syscall number, `rdi`, `rsi`, and `rdx`
contain arguments 1–3, and `rax` contains the result. The SDK assembly declares `rcx`
and `r11` clobbered. The IDT entry permits Ring 3; the kernel saves the canonical
register context shared with the timer and returns through `iretq` or schedules another
process. This uses a software interrupt, not x86-64 Linux's `syscall` instruction.

General failure is `SYSCALL_ERROR = u64::MAX - 1`, not negative POSIX `errno`.
Internal exit/block sentinels are handled by the kernel assembly and are not ordinary
successful user results. Wrapper results are typed as `Option`/`Result` where implemented;
raw wrappers such as `read` do not automatically turn every raw error into `Result`.

## Operations

Unused arguments are zero in SDK wrappers. `ptr` means a user virtual address.

| Number | SDK operation | Arguments (`rdi`, `rsi`, `rdx`) | Result/contract |
| --- | --- | --- | --- |
| 0 | `exit` | exit code | Terminates current process; no return |
| 1 | `console_write` | byte ptr, byte length | Bytes written; text renders to private surface unless client drawing enabled; also serial diagnostics |
| 2 | `read` | fd, buffer ptr, byte capacity | fd 0: focused keyboard input (zero after wake means retry); fd 3–1026: regular-file read at current offset, short count or zero at EOF |
| 3 | `ticks` | none | Monotonic PIT tick count, not milliseconds/wall time |
| 4 | `read_nb` | fd, buffer ptr, byte capacity | Nonblocking keyboard read; zero when no input or unfocused |
| 5 | `ls` | path ptr, output ptr, max entries | Entry count; each entry is 11 raw 8.3 filename bytes plus one attribute byte; directory bit `0x10` |
| 6 | `read_file` | path ptr, output ptr, byte capacity | Bytes read from a file by path |
| 7 | `write_file` | path ptr, input ptr, byte length | Bytes written; create/overwrite by path |
| 8 | `spawn` / `Child::spawn` | explicit executable path ptr, flags (0 ordinary, 1 connected stdio), zero | New PID; parent continues; static ELF/legacy flat image loaded from FAT32 |
| 9 | `kill` | PID | Zero on success; killing self does not resume the caller |
| 10 | `waitpid` | PID | Exit code; blocks while running, reaps zombie when applicable |
| 11 | `mouse` | none | Packed coordinates/buttons, decoded by SDK to `(x, y, buttons)` |
| 12 | `mkdir` | path ptr | Zero on success; create directory with missing parents |
| 13 | `fb_info` | output ptr (four aligned `u64` slots) | Writes width, height, pitch in bytes, bits per pixel; returns zero |
| 14 | `snapshot` | PID, output ptr, byte length | PID 0 only; nonempty length at most `pitch * height`; zero for live target, one for zombie; copies that prefix of the private surface |
| 15 | `focus` | PID | PID 0 only; selects keyboard recipient; returns zero |
| 16 | `graphics_mode` | none | Disables kernel console rendering on caller's surface; serial output remains; returns zero |
| 17 | `sleep_ticks` | duration in PIT ticks | Zero on wake; zero duration returns immediately; durations above `i64::MAX`, overflowing absolute deadlines, or no calling process return error |
| 18 | `present_surface` | none | Publishes an update to the caller's private surface by advancing its revision; zero on success, error if caller has no private surface |
| 19 | `surface_status` | PID | PID 0 only; revision in bits 1–62, zombie flag in bit 0; error for missing target or display owner (no private surface) |
| 20 | `send_message` | target PID (`u32`), payload (`u32`), zero | Zero on enqueue, one when full, error for forbidden/missing/dead target or invalid integer arguments |
| 21 | `receive_message` | zero, zero, zero | Zero if empty, otherwise `((sender + 1) << 32) \| payload`; error without a current process or with nonzero arguments |

| 22 | connected standard I/O (SDK `process` module) | operation (0 metadata, 1 send, 2 receive), scalar payload for send, endpoint selector | See connected-child contract below; no pointers |
| 23 | `open` | path ptr, flags, zero | Lowest free regular-file descriptor, 3–1026; see file contract below |
| 24 | `close` | fd, zero, zero | Zero on success; closed/invalid descriptors fail |
| 25 | `write` | fd, input ptr, byte length | Regular file: possibly short count, advances offset; fd 1/2: console output |
| 26 | `lseek` | fd, signed byte offset bits, origin | New byte offset; origin 0/1/2 = SET/CUR/END |
| 27 | `chdir` | path ptr, zero, zero | Zero on success; failure preserves process cwd |
| 28 | `getcwd` | output ptr, capacity in bytes, zero | Copies canonical absolute cwd plus NUL; returns bytes including NUL |
| 29 | legacy fixed-block argument spawn | path ptr, 2048-byte argument record ptr, spawn flags (0 ordinary, 1 connected I/O) | New PID; copies validated argv into private child stack before publication |

| 30 | `spawn_with_args` / `spawn_with_env` / connected `Child` equivalents | path ptr, request ptr, exact request size (48) | New PID; general argv/environment record below |
| 31 | `brk` (unsafe SDK primitive) | requested break, zero, zero | Zero request queries; success returns requested break, failure returns old break |
| 32 | `arch_prctl` / `fs_base`, `set_fs_base` | operation, address/output ptr, zero | Linux operation values `0x1002` SET_FS and `0x1003` GET_FS; zero on success |
| 33 | `fs::read_dir` | path ptr, output ptr, capacity bytes | Capacity zero queries required bytes; otherwise complete full-name records or error |
| 34 | `process::restrict_worker` | profile (1), heap bytes, zero | Irreversible restricted connected-child principal; see below |

Operations 14–16 currently appear as numeric dispatcher arms and numeric SDK calls.
Do not assume the named `SYS_*` constants above operation 13 are a complete ABI inventory.
Unknown numbers return `SYSCALL_ERROR`.

## Buffers, paths and geometry

The SDK builds a NUL-terminated UTF-8 path and rejects embedded NULs and strings
of 4096 bytes or more. The kernel copies through the terminator page by page,
validating only the bytes needed. VFAT components allow up to 255 UTF-16 code
units with compatible unique 8.3 aliases; illegal FAT characters, trailing spaces
and trailing dots are rejected. `/` selects root; an empty `ls` path selects cwd.
Other empty paths fail. Absolute paths start at root; relative paths use a snapshot
of process cwd. Repeated slashes and `.`/`..` are supported, clamped at root.
Intermediate directories must exist, including before `..`; recursive mkdir can
create missing parents. Canonical cwd preserves long names and uppercases short
names. Initial cwd is `/`; spawn inherits cwd and starts with no regular descriptors.
`STDIN_FD = 0` retains its keyboard/connected-input meaning.

For `ls`, callers must reserve **at least `12 * max_entries` bytes**; the SDK
checks this relationship before entering the kernel. For `fb_info`, ensure alignment
as well as length; the SDK uses `[u64; 4]`. For snapshots, the destination can be a
nonempty prefix of the surface, including row padding. Full snapshots remain
supported. Kernel paths validate full mappings and effective permissions. Requests
and file buffers are copied through owned kernel storage; snapshots use raw assembly
between stable owned surface frames and the validated destination. No user reference survives copying or
blocking. Siblings can change contents during a copy, so records are validated
after copying. Paths are copied before filesystem locks are acquired.

Mouse packing is x in bits 0–15, y in bits 16–31, buttons above bit 31. Button bits
0/1/2 mean left/right/middle. Coordinates are clamped to current display dimensions,
not historical 320×200 values found in some source comments. Query `fb_info`; honor
pitch, bounds and 32-bit XRGB format. Mapping at `0x410000000` is physical display memory
only for the compositor; other processes see private surface frames.

The current layout reserves image addresses below 4 GiB, a growable heap starting
at 4 GiB, the 4 MiB initial stack ending at 16 GiB, surface at `0x410000000`,
worker stacks at `0x420000000`, and static compiler TLS at `0x440000000`.
Use `userlib::sys::FRAMEBUFFER_ADDR`; rebuild all apps after this migration.
User mappings stay below `0x8000000000` (512 GiB); unused gaps are unmapped.
Surfaces cover `pitch * height`, including padding and the final partial page.
The kernel heap remains 64 MiB. See [address layout](architecture.md).

Graphical apps must call `present_surface()` after each rendering update. The WM
queries `surface_status()` and retains cached window pixels until the revision or
exit flag changes. Kernel console output publishes automatically; graphical serial
logs do not. Publication is a change notification, not atomic buffer swapping or
vsync. Existing numbers are preserved; snapshot accepts shorter buffers as well as
the original full surface. Rebuild graphical apps with the SDK when migrating.

`sleep_ticks()` deschedules the caller. Timer IRQs wake expired waits even while
the kernel idles; the scheduler uses `sti; hlt` when nothing can run. The PIT rate
remains approximately 18.2 Hz (~55 ms/tick), preserving game and cursor blink timing.
This reduces idle CPU but does not introduce a 60 Hz display/input loop.

## Program arguments and entry

Syscall 30 is the default SDK launch path. The shared
[process ABI](../crates/process-abi/src/lib.rs) defines a padding-free 48-byte
request: six little-endian u64 values `version=1, flags, argv, argc, envp, envc`.
Flags 0/1 select ordinary/connected standard I/O. Each vector points to
`StringRef { pointer: u64, length: u64 }` records containing UTF-8 without NULs.
Counts may independently reach 4096; combined string bytes plus one NUL per string
must fit 128 KiB. Empty vectors and strings are valid. Unknown flags/version,
invalid UTF-8, overflow, invalid mappings, embedded NULs or excessive sizes fail
before child publication. All records/strings are copied into owned kernel storage
before filesystem work or scheduling; no parent pointer survives into the child.

`sys::spawn_with_args(path, &["alpha", "two words", ""])` prepends path as argv[0]
and inherits a snapshot of `userlib::env`. `spawn_with_env(path, args, environment)`
uses explicit `KEY=value` strings. `Child` offers corresponding connected variants.
`env::var`, `set_var`, `remove_var` and `vars` manage a process-local environment;
parent/child mutations are independent. Raw callers choose argv[0] and environment
contents. SDK key mutation rejects empty keys, `=` in keys and NULs.

Legacy syscall 29 remains exactly a 2048-byte record: u64 version=1 and count,
then 2032 concatenated NUL-terminated UTF-8 bytes, at most 32 arguments. Its spawn
flags remain in RDX. The no-alloc SDK fallback uses this form without environment.
Legacy syscall 8 supplies the resolved path as its single argument. Direct kernel
boot spawning retains argc zero. There is no exec/fork or Linux binary compatibility.

The loader copies argc, terminated argv/envp pointer tables and NUL strings into
as many private final stack pages as required. RSP is eight bytes below that area
(8 modulo 16), RDI holds argc, RSI points to argv and RDX points to envp. This is
the native extern-C `_start` contract; there is no Linux auxiliary vector. Worker
entry still receives its single closure argument in RDI, with RSI zero.

The SDK captures these registers once before calling application code. Both
`fn main()` and `fn main(argc: usize, argv: &[&str])` work through the shared Cargo
root. `userlib::args()` iterates immutable string views without allocation;
`userlib::args::arg(index)` selects one argument. `userlib::argc()` and
`userlib::argv()` expose the count and raw `*const *const u8` table for C-style
access. Raw-pointer callers must not mutate the table/strings while shared string
views exist. Initial stack frames remain process-owned even if its first thread
exits; argument views remain valid while the process lives.

Arguments are copied into owned kernel storage before filesystem work can release
the entry gate. Child stack setup uses owned unpublished frames through the RAM
direct map, under the existing allocation transaction. No parent pointers survive
into the child. Allocation or later loader failures roll back private frames;
ordinary exit/kill/reap releases the process. Rebuild all applications when changing
this startup ABI. See [argument validation](arguments-validation.md).

## Regular files and working directories

The names, flags, byte offsets, short reads/writes and EOF behavior follow the
Linux [open](https://man7.org/linux/man-pages/man2/open.2.html),
[read](https://man7.org/linux/man-pages/man2/read.2.html) and
[lseek](https://man7.org/linux/man-pages/man2/lseek.2.html) interfaces where supported.
Existing syscall numbers and the generic native error value are preserved; this
is not Linux binary compatibility or negative `errno`. `getcwd` returns a byte
count as the Linux kernel syscall does, while the SDK returns a borrowed `&str`.
There is no mode/permission argument: open's third register must be zero.

Supported open flags use Linux values: `O_RDONLY=0`, `O_WRONLY=1`, `O_RDWR=2`,
`O_CREAT=0o100`, `O_TRUNC=0o1000`. Other bits, access mode 3 and read-only truncation
fail. Directories cannot be opened as regular files. Missing parents fail. A full
descriptor table fails before creation/truncation. Each open has an independent
cursor; threads in the same process share the table and each open's cursor.
Close/exit/kill release owned descriptors; an operation already holding an owned
reference may finish after concurrent close. Spawn inherits cwd, **not** descriptors.
No dup, fork, rename/unlink, descriptor inheritance, permissions or pipes are added.

Read/write validate complete advertised user buffers and use owned kernel data.
Each transfer is at most 64 KiB and may return a short count. Reading at or beyond
EOF returns zero. Read-only writes and write-only reads fail. Zero-length valid
file I/O returns zero without changing offsets. Seeking accepts a signed offset,
rejects negative results/overflow/unknown origins, and permits positions through
`u32::MAX`. Writing beyond EOF zero-fills the gap. Descriptor writes operate on
affected clusters without staging or rewriting the complete file; file sizes can
reach FAT32's `u32::MAX`, subject to disk space and allocation resources. Tables
grow to 1024 regular descriptors (3–1026) per process; no configurable rlimit exists.

Offset writes reserve any extension chain before modifying existing data. Disk-full
allocation failure preserves old bytes and frees the partial extension. Hardware
I/O failure can leave a partial in-place write; there is no journaling/fsync or
power-loss guarantee. Path-based whole-file writes still allocate a replacement
chain before publishing, requiring temporary space for both copies. Editor saves
use that replacement path so a failed allocation does not first truncate the old file.

Use `userlib::fs::File` for owned close-on-drop descriptors, `read`, `write_all`
and `seek`; low-level typed wrappers live in `userlib::sys`. `sys::write(1/2, ...)`
uses the existing SDK output redirection. The connected standard-I/O transport
remains separate from the regular-file table.

`cd` is parsed and executed by Forge. The kernel only validates and stores cwd;
Portal remains a terminal with no command parser. See
[commands and medit](terminal-shell.md#filesystem-commands-and-medit) and
[executed validation](file-commands-validation.md).

## Heap, thread pointer and full-name listing

`brk` owns zero-filled private user pages between 4 GiB and one guard page below
the main stack. Failed growth rolls back partial leaves/parent tables before
recycling frames after remote TLB invalidation. Shrink unmaps/recycles complete
pages; the containing partial page stays mapped. The SDK allocator serializes
break ownership and reuses freed blocks; mixing direct brk with live allocations
is unsafe. Physical RAM bounds allocations; there is no demand paging or swap.

`arch_prctl` saves FS base per native thread. SET_FS accepts zero or an address
with a mapped user-readable eight-byte range, and rejects noncanonical values.
GET_FS writes one u64 to fully validated writable user storage. SDK mutation is
unsafe because the caller must preserve live compiler TLS/TCB layout and lifetimes.
The loader initializes local-exec PT_TLS using x86-64 variant II, with initialized
TDATA, zeroed TBSS and a self pointer at FS:0. See [thread limits](threading.md#limits).

Full-name syscall 33 returns 8-byte-aligned records: u16 record bytes, u16 UTF-8
name bytes, u8 attributes, three zero reserved bytes, u64 file size, then UTF-8
name, NUL and zero padding. It returns total bytes; insufficient capacity fails
without copying a prefix. Legacy syscall 5 still returns 8.3 aliases. The SDK
queries size and retries boundedly if the directory grows (16 MiB listing budget).

## Display-owner message channel

Each PCB owns a fixed sixteen-entry FIFO of scalar messages. A client can send
only to PID 0; PID 0 can send to any live process. The kernel supplies sender
identity and does not interpret payloads. Full queues return backpressure without
overwriting earlier messages. Receive consumes one message and never blocks.
Reaping drops the inbox; sends to zombies or reaped processes fail. New calls use
only register values, so they introduce no user-pointer dereferences or retained
user references. Target/payload truncation and nonzero reserved args are rejected.

The SDK returns `MessageError::Full` or `MessageError::Unavailable`, and decodes
received values to `Option<(u32, u32)>`. PID 0 remains the existing privileged
display-owner exception; this is not general IPC permission/capability management.
The [Ring 3 window protocol](ui.md#window-protocol-version-1) uses this transport.
The [desktop shell extension](desktop-environment.md#shell-message-extension)
also runs entirely in userspace. The kernel does not interpret shell-role or
workspace messages; PID 0 authenticates its spawned DE and accepts/rejects requests.
Native clients retain immutable submitted surface pixels until the WM acknowledges
its snapshot copy. Existing syscalls 0–19 and legacy XRGB applications are preserved.

## Changing the interface

First add a failing kernel boundary test and a userspace caller test. Update the
kernel dispatcher, validation, SDK wrapper, application call sites, this table, and
error/boundary tests together. Rebuild every app and the boot disk. Prove behavior
in Ring 3, including rejected/unmapped/read-only buffers and privilege checks where
relevant. Keep existing numbers stable unless the task explicitly includes migration.
Do not expose kernel pointers, internal scheduling sentinels, or Rust layout without
an explicit representation and ABI contract.


## Network client sockets (version 2)

Syscalls 64–70 are open, connect, send, receive, close, poll and interface info.
The kernel dispatcher and userlib share [record definitions/constants](../crates/net-abi/src/lib.rs).
[The networking ABI](networking.md#socket-abi-version-2) specifies the complete
argument table, exact sizes, byte order, quotas, readiness bits and error encoding.
The typed SDK is [userlib::net](../user_programs/userlib/src/net.rs).

These are PID-owned generation socket handles, not POSIX file descriptors. ICMP
Echo, connected UDP and outgoing TCP are supported. All operations are nonblocking
and drive bounded protocol progress; callers sleep between retries. Every buffer
page and effective user/write permission is checked before copying. Network errors
use their own documented range below the existing control sentinels. Socket
close/exit/kill releases owned buffers. DNS/HTTP logic stays in userspace; the
kernel exposes no ping-command or HTTP-specific syscall.

## Platform randomness and UTC

The [shared platform ABI](../crates/platform-abi/src/lib.rs),
[kernel implementation](../src/system.rs), and
[typed SDK](../user_programs/userlib/src/system.rs) define these operations:

| Number | SDK operation | Arguments (`rdi`, `rsi`, `rdx`) | Result |
| --- | --- | --- | --- |
| 80 | `system::random_bytes` | writable output ptr, byte length (0–256), zero | Exact byte count, or platform error; SDK chunks larger slices into calls |
| 81 | `system::unix_time` | zero, zero, zero | UTC Unix seconds from validated CMOS, or platform error |

Both require a current process and reject nonzero reserved arguments. Zero-byte
random requests return zero without inspecting the pointer or accessing hardware.
Nonempty output must have effective present/user/writable permission at every
paging level and across every page, with checked address arithmetic and bounds
`0x400000..0x8000000000`. The kernel fills a bounded local record first and copies
only after complete entropy success. A failed syscall never partially writes its
output. A multi-call SDK request can have completed earlier chunks before a later
chunk fails; callers must not use it after an error.

RDRAND absence returns Unavailable; failure after ten attempts per 64-bit word
returns Failed. No fallback PRNG is seeded with ticks. Clock reads are bounded to
eight attempts for two matching stable samples, validate calendar/format, and
return Unavailable for invalid or inaccessible RTC data. UTC is host-controlled
wall time, not a monotonic clock; use `ticks` for deadlines. TLS additionally
rejects dates before 2020. These drivers do not provide a clock-setting API.

Platform errors encode as `u64::MAX - 511 + code`: Invalid=1, Unavailable=2,
Failed=3, distinct from network errors and scheduler control sentinels. Existing
syscall numbers and network ABI records remain unchanged. TLS, certificates,
trust policy and HTTPS are [userspace library responsibilities](tls.md), with no
TLS-specific kernel syscall.
## PCM audio output

Audio policy/decoding remains Ring3. The optional kernel AC97 driver exposes one
exclusive stereo signed16-bit little-endian48kHz stream, with opaque PID-owned
handles and32 bounded DMA descriptors. See [P6 ownership/testing](browser-media-p6.md).

| Number | SDK operation | Arguments (`rdi`, `rsi`, `rdx`) | Result |
| --- | --- | --- | --- |
|82 | `audio::Stream::open` |zero, zero, zero |Owned handle, or NoDevice/Busy/error |
|83 | `Stream::write` |handle, readable PCM ptr, byte length |Exact accepted bytes or WouldBlock/error; length4–4096, divisible by4 |
|84 | Stream control/Drop |handle, operation, zero |0 or error;0 close,1 pause,2 resume |
|85 | `Stream::status` |handle, writable status ptr,40 |0 or error; copies complete version1 record |

Status layout is padding-free: version u32, flags u32 (bit0 running,bit1 paused),
submitted frames u64, played frames u64, queued frames u32, free slots u32,
hardware FIFO-error count u64. Counters are complete stereo PCM frames, not bytes
or individual samples. Complete readable/writable mappings and reserved arguments
are validated before device access; no user pointer survives a copy. Audio errors
use `u64::MAX-1023+code` (Invalid1, NoDevice2, Busy3, WouldBlock4, Failed5,
NotOwner6); unknown reserved encodings fail closed in the SDK.

Close/exit/kill stops DMA and invalidates the lease, including owned child teardown.
Boot-owned33 DMA frames are reused for the controller lifetime. Reset failure
quarantines the controller and prevents buffer reuse. Restricted script workers
cannot invoke these calls. This is a native mechanism, not an OSS/ALSA/Linux ABI.

## Connected child standard I/O

`spawn` flags zero retains ordinary spawning. Flag one creates an owned duplex
connection before the child can run. Unknown flags, nonzero reserved arguments
and an occupied endpoint fail before loading a child. This is generic process I/O;
the kernel does not know whether the executable is a shell.

Syscall 22 operates only on the caller's existing endpoints. RDX=0 preserves the
original standard stream; RDX=`owned_child_pid + 1` explicitly selects the one
child connection owned by this caller. A connected child may itself own one
connected descendant, with a separate FIFO. Foreign PIDs, a parent PID and stale
selectors do not confer authority. Metadata (operation0) still requires RDX=0.
The SDK's `Child` always uses the explicit selector; stdin/stdout use zero.
Zero-selector stdio retains its original stream. Older SDK `Child` wrappers
cannot address a nested endpoint: rebuild them with the selected-endpoint SDK
before using this new topology. All repository applications were rebuilt together.
Existing ordinary host/child zero-selector traffic remains covered by regression tests.



- Operation 0: metadata, zero if unconnected, otherwise
  `((peer_pid + 1) << 1) | child_side`. A closed endpoint keeps metadata until reaped.
- Operation 1: enqueue a `u32` scalar in the peer's private 4096-entry FIFO;
  zero success, one full, general error if unavailable. Scalars are opaque to the kernel.
- Operation 2: consume the oldest incoming scalar, encoded as `u64(payload) + 1`.
  Zero means empty but live. General error means disconnected. Buffered packets
  can still be drained after the peer exits.
- Unknown operations, values above `u32::MAX`, nonzero unused/reserved arguments
  and operations on unavailable endpoints are rejected.

Queues are separate from syscalls 20/21 and cannot be addressed by arbitrary PID.
SDK byte framing uses count bits 24–31 and up to three low bytes in little-endian
order. Ordinary stdin host-to-child packets have count one. Explicit framed
protocols may use `Child::write_packet` and `process::read_packet` with canonical
one-to-three-byte packets. The sender returns a nonblocking prefix count (zero
for empty input, Full consumes nothing); the receiver distinguishes live-empty
from EOF and rejects invalid counts/nonzero unused bytes. Header and body packet
boundaries remain separate in the browser protocol. Ordinary `write_byte`/stdin
semantics are unchanged; the kernel still treats payloads as opaque words.
Standard console wrappers redirect
on the child side; the host uses `process::Child` and keeps ordinary keyboard I/O.
No new kernel pointer dereference or retained user reference is introduced by
operation 22. Spawn copies and validates the complete NUL-terminated path
before loading the executable through the filesystem service.

The host owns the child lifetime. Host exit/kill stops its owned connected descendants;
reaping the host also removes those stopped PCBs. Traversals use constant stack
space and are bounded by128 live process slots. Waiters are woken from leaf to
root; an already-exited descendant keeps its original exit status. Reaping a child releases its host
endpoint and drops queued packets. Killing a host from its own child schedules away
from the stopped child. Frames/stacks are reclaimed after deferred execution/TLB quiescence; existing
process-control permission limitations still apply. See [terminal architecture](terminal-shell.md).

A selected Child is nonblocking; the browser transport uses its existing bounded
sleep/retry/deadline loop for progress. Readiness syscall109's stdio bits continue
to monitor the original zero-selector stream. A child's stdout backpressure checks
the correct incoming FIFO in its parent, including a parent's descendant queue.
No new user pointer or syscall number is introduced. See
[P3 native JavaScript integration](browser-javascript-p3.md) for executed evidence.

## Native threads and CPU information

The [shared threading ABI](../crates/thread-abi/src/lib.rs),
[kernel dispatch](../src/userspace/syscall.rs), and
[typed Ring 3 SDK](../user_programs/userlib/src/thread.rs) define operations 96–109.
See [threading](threading.md#syscalls) for the register contracts, shared resources,
join/detach semantics, atomic-word waits, lifecycle and limits. Existing syscall
numbers and the `int 0x80` calling convention are unchanged. The custom target
still has no port of Rust `std`; enable the SDK's `threads` feature instead.
Mailbox readiness (108) is also exposed as `userlib::sys::wait_message` without
the threads feature. It parks only the caller, returns 1 for queued data or 0
on deadline, and leaves consumption and protocol interpretation in userspace.


Readiness waiting (109) takes a mask in RDI, timeout PIT ticks in RSI (zero means
indefinite), and reserved RDX=0. Bits are mailbox=1, connected-I/O read=2,
connected-I/O write=4, focused keyboard=8, display-owner pointer=16; zero/unknown bits and timeouts exceeding
`i64::MAX` fail. It returns ready bits, zero on timeout, or `SYSCALL_ERROR`.
Inspection and parking share the scheduler lock; queue changes wake readers and
backpressured writers immediately. EOF counts as readiness; I/O reports the final
status. It does not consume data or expose foreign endpoints/keyboard input.
`userlib::sys::{Events, wait_events, wait_stdin}` are available without the threads
feature. Readiness is a hint: competing same-process consumers must recheck I/O.

Pointer readiness compares a complete driver sample's published revision with the
display owner's last mouse read. Only PID 0 can select this bit. The existing
mouse syscall's packed return format is unchanged. IRQs publish complete samples
and send at most one idle-CPU doorbell; ordinary kernel entry performs wakeups.


## Restricted worker principal

Syscall 34 takes profile `1`, a page-aligned heap budget from 64 KiB through
64 MiB, and a zero reserved word. It returns zero or `SYSCALL_ERROR`. Only a
single-threaded non-PID-0 process with the **child** side of an existing connected
stdio endpoint and no owned descendant may enter it. A repeat transition, invalid budget/profile/flags,
or a budget below the current break fails without changing state. The budget is
relative to `HEAP_BASE`; it bounds complete brk pages, including rejected growth
rollback. Initial image/stack/surface/table allocations are separate.

Enforcement occurs before dispatch/usercopy for every syscall and thread. Allowed
numbers: 0, 3, 17, 22, 31, 32, 80, 81, 96, 97, 102, 103; syscall 109 allows only
nonzero combinations of stdio readiness bits 2/4. Other operations, including
unknown future numbers, return `SYSCALL_ERROR`. Thus existing endpoint ownership
remains authoritative; a worker cannot choose another peer, spawn descendants,
create threads, open files/sockets, access input/display or relax the profile.

`userlib::process::restrict_worker(heap_bytes)` is the typed wrapper. Call it from
trusted bootstrap before accepting untrusted bytes. It is a restricted-worker
mechanism, not origin isolation, JIT protection, a total-memory accounting ABI or
a promise that arbitrary existing resources are revoked/closed. Ordinary apps'
rights are unchanged. The native service bootstraps with an empty environment and
no pre-opened resources beyond its private endpoint. See
[P1 design and executed validation](browser-runtime-p1.md).

## Owned child supervision (version 1)

Operations 35–37 are native lifecycle mechanisms. They do not grant authority
through numeric PID alone. Typed wrappers are `userlib::supervision`.

| Number | Operation | Arguments | Result |
| --- | --- | --- | --- |
| 35 | current identity | all zero | generation in high 32 bits, PID in low 32 bits |
| 36 | consume child event | writable pointer, exactly 40 bytes, zero | 0 empty, 1 event, general error |
| 37 | terminate owned child | complete identity, zero, zero | 0 success, general error |

Current identities use generation 1; monotonically allocated PIDs are never
reused and exhaustion fails. A stale/unsupported generation is rejected. The
40-byte little-endian record contains version u32 at 0, event tag u32 at 4,
identity u64 at 8, cause u32 at 16, zero reserved u32 at 20, status u64 at 24,
and eight zero reserved bytes at 32. Version is 1, completed tag is 2; causes are
0 explicit exit, 1 forced termination, 2 fault. Adopted tag 1 reports a live child
reparented to native PID 1; it has zero cause and status. If that child has already
exited, completion supersedes adoption. No kernel address is included.

Readiness syscall 109 accepts CHILD bit 32, which means an owned child's
completion remains pending. It does not consume status. Invalid/unmapped/read-only
output or wrong size/reserved words leave the record untouched. Successful
consumption reaps exactly one owned zombie through deferred reclamation. The
new termination operation checks direct ownership, complete identity and live
state under scheduler serialization. Legacy-profile kill/wait remains permissive
for the current compositor. The native profile checks parent ownership through these operations too. Profile
selection is immutable after process publication. Native orphan adoption retains
both live notifications and zombie statuses; the manager must terminate/reap
unconfigured live orphans. Native GRUB bootstrap starts Initator as PID 1; its
exit/fault/last-thread exit parks the remaining CPUs and halts with allocations retained. This is a trusted
service profile, not a complete capability or multiuser isolation system.
Both profiles reject PID values above `u32::MAX` and nonzero unused arguments to
kill/wait before side effects. See [M1 evidence](initator-progress.md).
