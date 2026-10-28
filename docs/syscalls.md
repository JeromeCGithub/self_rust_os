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
| 14 | `snapshot` | PID, output ptr, byte length | PID 0 only; length must equal `pitch * height`; zero for live target, one for zombie; copies private surface |
| 15 | `focus` | PID | PID 0 only; selects keyboard recipient; returns zero |
| 16 | `graphics_mode` | none | Disables kernel console rendering on caller's surface; serial output remains; returns zero |

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
as well as length; the SDK uses `[u64; 4]`. For snapshots, the destination is a full
surface buffer. Do not form kernel slices from hostile pointers without verifying
mapping, permissions, overflow, alignment and lifetime; current numerical-range checks
are incomplete hardening, as noted in [architecture](architecture.md).

Mouse packing is x in bits 0–15, y in bits 16–31, buttons above bit 31. Button bits
0/1/2 mean left/right/middle. Coordinates are clamped to current display dimensions,
not historical 320×200 values found in some source comments. Query `fb_info`; honor
pitch, bounds and 32-bit XRGB format. Mapping at `0x900000` is physical display memory
only for the compositor; other processes see private surface frames.

## Changing the interface

First add a failing kernel boundary test and a userspace caller test. Update the
kernel dispatcher, validation, SDK wrapper, application call sites, this table, and
error/boundary tests together. Rebuild every app and the boot disk. Prove behavior
in Ring 3, including rejected/unmapped/read-only buffers and privilege checks where
relevant. Keep existing numbers stable unless the task explicitly includes migration.
Do not expose kernel pointers, internal scheduling sentinels, or Rust layout without
an explicit representation and ABI contract.
