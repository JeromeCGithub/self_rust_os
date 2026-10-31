# Architecture and privilege boundaries

`self_rust_os` is an experimental x86-64 OS written in `no_std` Rust. The GRUB
QEMU platform supports 1–32 logical CPUs and native Ring 3 threads; bootimage
harnesses retain their single-CPU path. See [threading and SMP](threading.md).
The responsibility split resembles Linux: a privileged kernel supplies mechanisms,
and unprivileged applications implement policy. This is **not Linux**, is not
POSIX-compatible, and does not execute Linux ELF applications or expose Linux syscalls.
Drivers currently live in the kernel; the userspace compositor does not make it a
microkernel.

## Ownership map

| Layer | Location | Responsibilities |
| --- | --- | --- |
| Kernel entry/boot | [entry](../src/entry_point.rs), [GRUB entry](../src/grub_boot.rs), [boot helpers](../src/boot/mod.rs) | Boot metadata, mode transition, physical mappings, bring-up |
| Kernel CPU/interrupts | [CPU architecture](../src/arch/x86_64/mod.rs), [GDT/TSS](../src/arch/x86_64/gdt.rs), [interrupts](../src/arch/x86_64/interrupts.rs) | Privilege selectors, stacks, IRQ delivery, exceptions, preemption |
| Kernel memory | [frames/paging](../src/memory.rs), [heap](../src/allocator.rs) | Memory ownership, page tables, allocation |
| Kernel process management | [process/thread ownership](../src/process/mod.rs), [scheduler](../src/scheduler.rs), [loader](../src/userspace/process.rs) | Process state, private address spaces, contexts, blocking and wakeup |
| Kernel syscall boundary | [dispatcher](../src/userspace/syscall.rs) | Argument validation, privilege enforcement, resource operations |
| Kernel device drivers | [driver tree](../src/drivers/mod.rs) | Device discovery, register access, DMA buffer ownership, block/Ethernet/input/framebuffer transports |
| Kernel storage | [MBR](../src/mbr.rs), [FAT32](../src/fat32.rs), [mount](../src/fs.rs) | Partition offsets and filesystem mechanisms above the ATA driver |
| Kernel networking | [facade](../src/net/mod.rs), [protocol service](../crates/net-stack/src/lib.rs) | Ethernet/ARP/IPv4 protocols, ICMP/UDP/TCP, bounded progress, PID/socket ownership and syscall checks above the Ethernet driver |
| Kernel console | [console](../src/console.rs) | Boot text console using the framebuffer driver |
| User SDK/runtime | [userlib](../user_programs/userlib/src/lib.rs), [syscalls](../user_programs/userlib/src/sys.rs), [UI](../user_programs/userlib/src/ui/mod.rs), [runtime](../user_programs/userlib/src/rt.rs) | Ring 3 syscall wrappers, windows, canvas, widgets, `_start`, panic handler, console formatting |
| User compositor | [wm](../user_programs/wm/src/main.rs) | Stacking, placement, decorations, focus/input routing, surface ownership, glass and presentation |
| Desktop environment | [de](../user_programs/de/src/main.rs) | Wallpaper, panels, launchers, application discovery and executable launch policy; requests compositor services |
| User shells/apps | [portal](../user_programs/portal/src/main.rs), [forge](../user_programs/forge/src/main.rs), [hello](../user_programs/hello/src/main.rs), [snake](../user_programs/snake/src/main.rs), [spin](../user_programs/spin/src/main.rs) | Commands, UI, executable selection, application behavior |
| Host tooling | [tools](../tools), [E2E](../e2e) | Building images, QEMU execution and verification; never guest privilege code |

**`src/userspace/` runs in Ring 0.** Its name means “kernel support for userspace.”
Everything in `user_programs/`, including `userlib`, executes in Ring 3. Each program
is a separate Cargo crate; the root manifest is not a multi-crate workspace.

## Drivers and core kernel services

Drivers are part of the privileged monolithic kernel, as in Linux. The directory
boundary expresses responsibility; it adds no process, address-space, or fault
isolation. Core services own scheduling, memory allocation, page tables,
filesystems, network protocols, and syscall validation. Drivers own device
registers and transport state and use core memory services to reserve and map
hardware resources. Drivers must not implement shell or application policy.

```text
src/drivers/
  block/ata.rs              ATA PIO sector transport
  char/random.rs            Bounded RDRAND entropy
  rtc/cmos.rs               Stable PC CMOS UTC samples
  pci/mod.rs                PCI configuration access, discovery and command bits
  net/e1000.rs              Intel 82540EM registers, DMA rings, Ethernet frames
  input/keyboard.rs         PS/2 keyboard setup and scancode queue
  input/mouse/mod.rs        PS/2 mouse setup and packet decoding
  input/mouse/absolute.rs   QEMU VMware-compatible absolute pointer
  tty/serial.rs             16550 diagnostic serial output
  video/framebuffer.rs      Boot-provided framebuffer and drawing primitives
```

The network service consumes the Ethernet driver's smoltcp `Device` interface;
it owns protocol configuration and process sockets. FAT32 and partition code
consume block operations from the ATA driver. Input IRQ dispatch stays in
`interrupts.rs`, which owns IDT entries, PIC acknowledgement and scheduler wakeup;
the input drivers consume device bytes. CPU-specific GDT, timer/context-switch and
PIC routing code remains architecture glue in the existing core modules.

Boot calls driver initialization explicitly. The framebuffer driver retains boot
console drawing helpers and input retains the legacy asynchronous scancode helper.
This is an initial Linux-style source layout and responsibility split: there is
no loadable-module ABI, Linux driver API, common device registry or hotplug support.
Future hardware drivers belong under the appropriate `drivers/` subsystem.

## Boot and process lifecycle

[The runner](../tools/run-desktop.sh) routes ordinary `cargo run` to a GRUB disk
build and QEMU, and routes test executables to `bootimage runner`. GRUB supplies a
1920×1080×32 framebuffer. The kernel establishes memory mappings, GDT/IDT/PICs, heap,
input queues, and the FAT32 root mount. It loads `/BIN/WM.BIN`, creates the initial
PID 0 process, and enters Ring 3 through the scheduler. See [boot details](grub-boot.md).
The compositor then spawns `/BIN/DE.BIN` in Ring 3. That process requests a desktop
surface and panel work-area reservations over the scalar message channel; no
shell-widget rendering or application-discovery policy executes in PID 0. See [the split](desktop-environment.md).

A process owns a private page-table subtree, code/data, drawing surface and shared
resource identity. Its native threads each own a guarded user stack, kernel stack
and saved register context. A shared ready queue schedules threads on available
CPUs, with optional worker affinity. Each CPU owns its current TID, TSS/GDT,
exception stacks, local timer and idle stack. IRQs that interrupt kernel code do
not preempt a lock holder. The fair CPU entry gate spans context handoff; waiting
CPUs halt with interrupts enabled and receive targeted wake IPIs. Filesystem work
uses owned buffers and its own service lock outside this gate. Keyboard IRQs queue
bounded work; ordinary scheduler entry consumes pending wakeups.

Blocking reads, waits and sleeps affect the calling thread. Thread states are
`Ready`, `Running`, `Blocked` and `Zombie`; process lifetime is `Live`/`Zombie`.
Process exit/kill stops all siblings, and `waitpid` reaps the PCB. Owned frames,
page tables and kernel stacks are deferred until no CPU is executing them and
remote TLB invalidation has completed. User page faults and general-protection
faults terminate the affected process; supervisor faults remain fatal.

Current virtual layout, defined in [kernel constants](../src/userspace/mod.rs):

| Region | Current address |
| --- | --- |
| Flat user binary entry | `0x400000` |
| User stack bottom/top | `0x2c00000` / `0x3000000` (4 MiB) |
| Application framebuffer/surface | `0x3100000`, `pitch * height` bytes rounded up to pages |
| Legacy drawing-surface limit | `0x4000000` (exclusive) |
| Guarded worker-stack arena | Starts at `0x4000000`, 63 slots of `0x41000` bytes each |
| Private user page-table region | `0x400000` through `0x6000000` (exclusive); gaps stay unmapped |
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
caches window content (up to 640×400 for native SDK clients), combines changed windows in
stacking order into a retained back buffer, and presents dirty 32×32 tiles. Cursor
movement restores the old cursor tiles from that buffer. Graphical clients publish
surface revisions after drawing; console output publishes automatically. Native
SDK clients also send frame serials and wait for copy acknowledgements before
reusing their pixels. Unchanged surfaces require only metadata queries. The six
cache slots reserve 5.86 MiB, plus 151 KiB of glass blur scratch storage, in
addition to the pitched composition buffer, full-resolution packed desktop cache,
and bounded 480-row application snapshot scratch. The DE's full-display surface
is copied before release and does not consume an application window slot.
Timed waits let the CPU halt when
applications are waiting. See [performance validation](performance.md). Text apps
receive kernel-rendered console output on their private surface. Graphical apps
call `graphics_mode()` (automatically through the window SDK); their console writes
remain serial diagnostics. The WM renders glass materials from lower layers;
widgets and layout stay in the client. See [the UI API and protocol](ui.md).

The WM decides which application has keyboard focus; the kernel enforces it.
Mouse input uses QEMU's VMware-compatible absolute protocol when available, otherwise
PS/2 relative packets. Input transport belongs in the kernel; desktop hit-testing,
window dragging, application discovery and theme belong in userspace.

## Storage and policy

GRUB boots from the primary master's first FAT32 partition. The root mount uses
MBR-relative sector offsets. Files use FAT32 8.3 names and directory traversal;
the current root filesystem is not the old ramfs described in historical notes.

The disk builder installs `/BIN`, `/ETC`, `/HOME`, and `/TMP`. The DE discovers
launchers from `/BIN` and reads `/ETC/WALL.PPM`; the compositor understands neither
the wallpaper path/format nor executable names. Portal hosts Forge over an owned standard-I/O connection; terminal rendering and
shell execution live in different processes. See [terminal architecture](terminal-shell.md).
Shells decide command semantics.
`spawn` receives an explicit path. Do not add PATH lookup, wallpaper selection,
or application-specific filename handling to a generic driver or syscall.

## Networking and application protocols

The optional QEMU e1000 device feeds a kernel smoltcp stack through a polled
Ethernet adapter. Link protocols (Ethernet/ARP), Internet mechanisms (IPv4/routing/
ICMP), transport state (UDP/TCP), and process/socket validation have separate
responsibilities. A common padding-free ABI crosses int 0x80 into typed Ring 3
SDK calls. Forge's ping/curl policy, DNS resolution and the HTTP SDK stay in userspace.
See [networking](networking.md) for the layer diagram, exact syscall contracts,
execution model and resource limits.

Network syscalls validate actual user mappings and effective page permissions,
copy complete records, and retain no user references. File, console, graphics
and threading calls also use complete mapping checks and assembly copies.
Metadata and file buffers use owned kernel storage; surface snapshots copy from
stable owned frames without borrowing client memory. Socket state is reclaimed on close/exit/kill;
process frames and stacks are reclaimed after execution and TLB quiescence. Network progress is bounded and driven
by callers; there is no background network worker. Static IPv4, connected UDP and
outgoing TCP are supported. The [HTTP SDK and curl](http.md) implement DNS A
resolution and streaming HTTP/HTTPS GET in Ring 3. [TLS](tls.md) uses Rustls,
WebPKI verification and a RustCrypto provider; platform syscalls 80/81 supply
hardware entropy and UTC. TLS-enabled apps reserve a private 16 MiB BSS heap; all
initial process threads receive 4 MiB user stacks with unmapped guards. Kernel stacks remain
16 KiB. Server socket interfaces remain future work. Independent host fixtures verify framing,
redirects, response streaming and cleanup through real socket syscalls.

The loader queries file size and stages only the bounded actual binary, returning
an error on allocation failure. This permits spawning with live socket buffers
without requiring a contiguous maximum-size user image allocation. A blocked
waitpid receives the exit code in its saved return register and reaps the child.

## Existing limitations and design obligations

- Initial WM path and PID 0 compositor privileges are explicit boot/protocol
  exceptions, not general capability or session management. Other process-control
  syscalls are not a full permissions model.
- Desktop rendering supports 640×480 through 1920×1080 at 32 bpp, with pitch up to
  8192 bytes, and allows six simultaneous windows. The default boot requests 1080p.
  Legacy content remains 488×288 pixels; native SDK clients request content sizes
  from 160×96 through 640×400. One mapped window per process is currently exposed.
- Reclamation is deferred while remote CPUs execute an exiting context or finish
  an in-flight filesystem operation. Live/unreaped objects have bounded quotas;
  callers should join/detach threads and reap processes. See [limits](threading.md#limits).
- User copies validate every effective mapping and use byte assembly without
  borrowing shared user RAM. This is not a claim of general memory safety: disk
  binaries still lack complete W^X enforcement, process-control permissions remain
  limited, and supported hardware is the declared QEMU xAPIC platform.
- The async executor remains in `src/task/`; it is not the current desktop process
  scheduler. Do not confuse cooperative async tasks with preemptive Ring 3 processes.
- Legacy bootimage testing is distinct from the real GRUB desktop path. Neither
  successful compilation nor constants tests prove Ring 3 execution or isolation.

New interfaces must keep the kernel/userspace split, define ownership/error behavior,
and prove relevant invariants with tests. See [development](development.md),
[testing](testing.md), and [the ABI](syscalls.md).
