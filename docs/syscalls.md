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
| 2 | `read` | fd, buffer ptr, byte capacity | Blocking keyboard read; only fd 0 supported; focused process only |
| 3 | `ticks` | none | Monotonic PIT tick count, not milliseconds/wall time |
| 4 | `read_nb` | fd, buffer ptr, byte capacity | Nonblocking keyboard read; zero when no input or unfocused |
| 5 | `ls` | path ptr, output ptr, max entries | Entry count; each entry is 11 raw 8.3 filename bytes plus one attribute byte; directory bit `0x10` |
| 6 | `read_file` | path ptr, output ptr, byte capacity | Bytes read from a file by path |
| 7 | `write_file` | path ptr, input ptr, byte length | Bytes written; create/overwrite by path |
| 8 | `spawn` | explicit executable path ptr | New PID; parent continues; flat binary loaded from FAT32 |
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
supported. Do not form kernel slices from hostile pointers without verifying
mapping, permissions, overflow, alignment and lifetime; current numerical-range checks
are incomplete hardening, as noted in [architecture](architecture.md).

Mouse packing is x in bits 0–15, y in bits 16–31, buttons above bit 31. Button bits
0/1/2 mean left/right/middle. Coordinates are clamped to current display dimensions,
not historical 320×200 values found in some source comments. Query `fb_info`; honor
pitch, bounds and 32-bit XRGB format. Mapping at `0x2100000` is physical display memory
only for the compositor; other processes see private surface frames.

The 1080p layout moves the stack top from `0x800000` to `0x2000000` and the drawing
surface from `0x900000` to `0x2100000`, making room for compositor buffers below the
stack. Apps use `userlib::sys::FRAMEBUFFER_ADDR`; rebuild all apps with
`tools/build-user.sh` when migrating from the old layout. Syscall numbers and
`fb_info`/`snapshot` formats are unchanged. Surfaces cover `pitch * height`, including
padding and the final partial page. User mappings occupy private page tables below
`0x4000000`; unused gaps are unmapped. The kernel heap is 32 MiB to stage flat images.

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

## Changing the interface

First add a failing kernel boundary test and a userspace caller test. Update the
kernel dispatcher, validation, SDK wrapper, application call sites, this table, and
error/boundary tests together. Rebuild every app and the boot disk. Prove behavior
in Ring 3, including rejected/unmapped/read-only buffers and privilege checks where
relevant. Keep existing numbers stable unless the task explicitly includes migration.
Do not expose kernel pointers, internal scheduling sentinels, or Rust layout without
an explicit representation and ABI contract.


## Network client sockets (version 1)

Syscalls 20–26 are open, connect, send, receive, close, poll and interface info.
The kernel dispatcher and userlib share [record definitions/constants](../crates/net-abi/src/lib.rs).
[The networking ABI](networking.md#socket-abi-version-1) specifies the complete
argument table, exact sizes, byte order, quotas, readiness bits and error encoding.
The typed SDK is [userlib::net](../user_programs/userlib/src/net.rs).

These are PID-owned generation socket handles, not POSIX file descriptors. ICMP
Echo, connected UDP and outgoing TCP are supported. All operations are nonblocking
and drive bounded protocol progress; callers sleep between retries. Every buffer
page and effective user/write permission is checked before copying. Network errors
use their own documented range below the existing control sentinels. Socket
close/exit/kill releases owned buffers. DNS/HTTP logic stays in userspace; the
kernel exposes no ping-command or HTTP-specific syscall.
