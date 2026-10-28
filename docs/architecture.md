# Architecture and privilege boundaries

`self_rust_os` is an experimental, single-CPU x86-64 OS written in `no_std` Rust.
The responsibility split resembles Linux: a privileged kernel supplies mechanisms,
and unprivileged applications implement policy. This is **not Linux**, is not
POSIX-compatible, and does not execute Linux ELF applications or expose Linux syscalls.
Drivers currently live in the kernel; the userspace compositor does not make it a
microkernel.

## Ownership map

| Layer | Location | Responsibilities |
| --- | --- | --- |
| Kernel entry/boot | [entry](../src/entry_point.rs), [GRUB entry](../src/grub_boot.rs), [boot helpers](../src/boot/mod.rs) | Boot metadata, mode transition, physical mappings, bring-up |
| Kernel CPU/interrupts | [GDT/TSS](../src/gdt.rs), [interrupts](../src/interrupts.rs) | Privilege selectors, stacks, IRQ delivery, exceptions, preemption |
| Kernel memory | [frames/paging](../src/memory.rs), [heap](../src/allocator.rs) | Memory ownership, page tables, allocation |
| Kernel process management | [scheduler](../src/scheduler.rs), [loader](../src/userspace/process.rs) | Process state, private address spaces, contexts, blocking and wakeup |
| Kernel syscall boundary | [dispatcher](../src/userspace/syscall.rs) | Argument validation, privilege enforcement, resource operations |
| Kernel storage | [ATA](../src/ata.rs), [MBR](../src/mbr.rs), [FAT32](../src/fat32.rs), [mount](../src/fs.rs) | Block I/O, partition offsets, directory/file mechanisms |
| Kernel input/display | [mouse](../src/mouse.rs), [absolute pointer](../src/mouse/absolute.rs), [keyboard](../src/task/keyboard.rs), [graphics](../src/gfx.rs) | Hardware transport, input delivery, display mappings and boot console |
| User SDK/runtime | [userlib](../user_programs/userlib/src/lib.rs), [syscalls](../user_programs/userlib/src/sys.rs), [runtime](../user_programs/userlib/src/rt.rs) | Ring 3 syscall wrappers, `_start`, panic handler, console formatting |
| User compositor | [wm](../user_programs/wm/src/main.rs) | Wallpaper, icons, application discovery, stacking, decorations, focus policy, presentation |
| User shells/apps | [portal](../user_programs/portal/src/main.rs), [forge](../user_programs/forge/src/main.rs), [hello](../user_programs/hello/src/main.rs), [snake](../user_programs/snake/src/main.rs), [spin](../user_programs/spin/src/main.rs) | Commands, UI, executable selection, application behavior |
| Host tooling | [tools](../tools), [E2E](../e2e) | Building images, QEMU execution and verification; never guest privilege code |

**`src/userspace/` runs in Ring 0.** Its name means “kernel support for userspace.”
Everything in `user_programs/`, including `userlib`, executes in Ring 3. Each program
is a separate Cargo crate; the root manifest is not a multi-crate workspace.

## Boot and process lifecycle

[The runner](../tools/run-desktop.sh) routes ordinary `cargo run` to a GRUB disk
build and QEMU, and routes test executables to `bootimage runner`. GRUB supplies a
640×480×32 framebuffer. The kernel establishes memory mappings, GDT/IDT/PICs, heap,
input queues, and the FAT32 root mount. It loads `/BIN/WM.BIN`, creates the initial
PID 0 process, and enters Ring 3 through the scheduler. See [boot details](grub-boot.md).

A process has a private page-table subtree, user code/data and stack, a kernel stack,
and a saved register context. Timer preemption switches user processes; IRQs that
interrupt kernel code do not preempt lock-holding kernel paths. Blocking reads and
waits mark a process blocked; keyboard input and process exit wake waiters. States
are `Ready`, `Running`, `Blocked`, and `Zombie`; `waitpid` reaps exited PCBs.

Current virtual layout, defined in [kernel constants](../src/userspace/mod.rs):

| Region | Current address |
| --- | --- |
| Flat user binary entry | `0x400000` |
| User stack bottom/top | `0x7fc000` / `0x800000` (16 KiB) |
| Application framebuffer/surface | `0x900000`, currently `640 * 480 * 4` bytes |
| GRUB physical-memory offset | `0xffff800000000000` |
| GRUB kernel framebuffer mapping | `0xffff900000000000` |

These are project-specific mappings, not Linux's process layout. Flat user binaries
are linked at a fixed address. The builder materializes `.bss` into the flat image;
metadata records the read-only section boundary. Disk-spawned processes currently
use a read-only boundary of zero, so metadata generation alone does not establish
read-only code or W^X enforcement. No ELF loader, `fork`/`exec`, signals, or per-process
file-descriptor table is exposed by the current syscall interface.

## Display and input ownership

PID 0 owns the hardware framebuffer mapping. Other processes map private backing
frames at the same user virtual address. The compositor snapshots those frames,
combines windows in stacking order into a back buffer, then presents it. Text apps
receive kernel-rendered console output on their private surface. Graphical apps
call `graphics_mode()`; their console writes remain serial diagnostics.

The WM decides which application has keyboard focus; the kernel enforces it.
Mouse input uses QEMU's VMware-compatible absolute protocol when available, otherwise
PS/2 relative packets. Input transport belongs in the kernel; desktop hit-testing,
window dragging, application discovery and theme belong in userspace.

## Storage and policy

GRUB boots from the primary master's first FAT32 partition. The root mount uses
MBR-relative sector offsets. Files use FAT32 8.3 names and directory traversal;
the current root filesystem is not the old ramfs described in historical notes.

The disk builder installs `/BIN`, `/ETC`, `/HOME`, and `/TMP`. The WM discovers
launchers from `/BIN` and reads `/ETC/WALL.PPM`; shells decide command semantics.
`spawn` receives an explicit path. Do not add PATH lookup, wallpaper selection,
or application-specific filename handling to a generic driver or syscall.

## Existing limitations and design obligations

- Initial WM path and PID 0 compositor privileges are explicit boot/protocol
  exceptions, not general capability or session management. Other process-control
  syscalls are not a full permissions model.
- Desktop rendering currently assumes 640×480 and allows six simultaneous windows.
- Address-space frames and leaked kernel stacks are not reclaimed on process exit;
  repeated launch/close can exhaust memory. Reaping a PCB does not reclaim all resources.
- User-pointer checks include numerical range checks; these do not prove every page
  is mapped, has suitable permissions, or satisfies Rust reference lifetime rules.
  Treat this as incomplete hardening, not as an established safe boundary.
- The async executor remains in `src/task/`; it is not the current desktop process
  scheduler. Do not confuse cooperative async tasks with preemptive Ring 3 processes.
- Legacy bootimage testing is distinct from the real GRUB desktop path. Neither
  successful compilation nor constants tests prove Ring 3 execution or isolation.

New interfaces must keep the kernel/userspace split, define ownership/error behavior,
and prove relevant invariants with tests. See [development](development.md),
[testing](testing.md), and [the ABI](syscalls.md).
