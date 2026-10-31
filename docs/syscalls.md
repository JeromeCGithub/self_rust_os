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
| 2 | `read` | fd, buffer ptr, byte capacity | Keyboard read; only fd 0 supported; focused process only; after blocking/wakeup, zero means retry (the wake itself does not fill the buffer) |
| 3 | `ticks` | none | Monotonic PIT tick count, not milliseconds/wall time |
| 4 | `read_nb` | fd, buffer ptr, byte capacity | Nonblocking keyboard read; zero when no input or unfocused |
| 5 | `ls` | path ptr, output ptr, max entries | Entry count; each entry is 11 raw 8.3 filename bytes plus one attribute byte; directory bit `0x10` |
| 6 | `read_file` | path ptr, output ptr, byte capacity | Bytes read from a file by path |
| 7 | `write_file` | path ptr, input ptr, byte length | Bytes written; create/overwrite by path |
| 8 | `spawn` / `Child::spawn` | explicit executable path ptr, flags (0 ordinary, 1 connected stdio), zero | New PID; parent continues; flat binary loaded from FAT32 |
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

| 22 | connected standard I/O (SDK `process` module) | operation (0 metadata, 1 send, 2 receive), scalar payload for send, zero | See connected-child contract below; no pointers |

Operations 14–16 currently appear as numeric dispatcher arms and numeric SDK calls.
Do not assume the named `SYS_*` constants above operation 13 are a complete ABI inventory.
Unknown numbers return `SYSCALL_ERROR`.

## Buffers, paths and geometry

The SDK builds a zero-padded 256-byte, NUL-terminated UTF-8 path buffer and rejects
path strings of 255 bytes or more. Kernel helpers currently inspect a full 256-byte
user range for paths. Names use FAT32 8.3 conventions; `/` and an empty directory path
select the root. There is no current working-directory syscall, open/close syscall,
POSIX file descriptor model, or old ramfs API here. `STDIN_FD = 0` is keyboard input.

For `ls`, callers must reserve **at least `12 * max_entries` bytes**; the current SDK
wrapper does not establish that relationship itself. For `fb_info`, ensure alignment
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
pitch, bounds and 32-bit XRGB format. Mapping at `0x3100000` is physical display memory
only for the compositor; other processes see private surface frames.

The 1080p layout moves the stack top from `0x800000` to `0x3000000` and the drawing
surface from `0x900000` to `0x3100000`, making room for compositor buffers below the
stack. Apps use `userlib::sys::FRAMEBUFFER_ADDR`; rebuild all apps with
`tools/build-user.sh` when migrating from the old layout. Syscall numbers and
`fb_info`/`snapshot` formats are unchanged. Surfaces cover `pitch * height`, including
padding and the final partial page. User mappings occupy private page tables below
`0x6000000`; unused gaps are unmapped. The surface region still ends at
`0x4000000`, where the separate guarded worker-stack arena begins. The kernel heap is 64 MiB to stage flat images and service concurrent allocations.

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
`0x400000..0x6000000`. The kernel fills a bounded local record first and copies
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
## Connected child standard I/O

`spawn` flags zero retains ordinary spawning. Flag one creates an owned duplex
connection before the child can run. Unknown flags, nonzero reserved arguments
and an occupied endpoint fail before loading a child. This is generic process I/O;
the kernel does not know whether the executable is a shell.

Syscall 22 operates only on the caller's endpoint:

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
order. Host-to-child packets have count one. Standard console wrappers redirect
on the child side; the host uses `process::Child` and keeps ordinary keyboard I/O.
No new kernel pointer dereference or retained user reference is introduced by
operation 22. Spawn copies and validates the complete fixed-size path buffer
before loading the executable through the filesystem service.

The host owns the child lifetime. Host exit/kill stops its connected child;
reaping the host also removes that child's PCB. Reaping a child releases its host
endpoint and drops queued packets. Killing a host from its own child schedules away
from the stopped child. Frames/stacks are reclaimed after deferred execution/TLB quiescence; existing
process-control permission limitations still apply. See [terminal architecture](terminal-shell.md).

## Native threads and CPU information

The [shared threading ABI](../crates/thread-abi/src/lib.rs),
[kernel dispatch](../src/userspace/syscall.rs), and
[typed Ring 3 SDK](../user_programs/userlib/src/thread.rs) define operations 96–108.
See [threading](threading.md#syscalls) for the register contracts, shared resources,
join/detach semantics, atomic-word waits, lifecycle and limits. Existing syscall
numbers and the `int 0x80` calling convention are unchanged. The custom target
still has no port of Rust `std`; enable the SDK's `threads` feature instead.
Mailbox readiness (108) is also exposed as `userlib::sys::wait_message` without
the threads feature. It parks only the caller, returns 1 for queued data or 0
on deadline, and leaves consumption and protocol interpretation in userspace.
