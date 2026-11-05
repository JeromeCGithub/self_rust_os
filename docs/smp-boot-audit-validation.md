# SMP boot handoff and device namespace validation

This follow-up is stacked on `feat/native-threads-smp` at `f86b310`. It audits the
existing execution model and adds the bootloader ACPI handoff plus the first
usable `/dev` endpoints. It does not replace the scheduler or remove the entry gate.

## Audit findings and changes

| Requested mechanism | Current implementation and verification |
| --- | --- |
| BSP boot and firmware handoff | GRUB enters the BSP; Multiboot ACPI tags 14/15 supply an owned root-table selection. Checksums and sizes are checked; valid extended tags take precedence in either order. BIOS scanning is compatibility fallback only when no ACPI tag exists. |
| CPU discovery | Validated RSDT/XSDT leads to MADT. Enabled xAPIC IDs are discovered, the actual BSP is moved to dense index zero, and the supported range remains 1–32. |
| AP startup | BSP reserves a low-memory trampoline, allocates CPU-lifetime idle stacks, publishes CR3/stack/entry parameters, sends INIT/deassert/SIPI with a bounded retry, and waits for each acknowledgement. |
| Per-CPU context | Each CPU installs its own GDT/TSS, exception/NMI stacks and idle stack; the scheduler owns a distinct current TID per CPU. QEMU register assertions require unique GDT and TSS addresses. |
| Timers and IPIs | AP acknowledgement now follows local timer setup, with IRQs still disabled. Calibrated LAPIC timers schedule APs; BSP PIT maintains public time. Targeted fixed IPIs wake work/gate waiters and NMI shootdowns enforce TLB quiescence. |
| SMP execution | Pinned native workers invoke kernel syscalls on each CPU. Two workers sharing the last CPU rendezvous without a syscall or voluntary yield in their spin loop, proving timer preemption. Existing lifecycle/fault/exhaustion checks remain. |
| Idle and kernel work | Idle APs mask scheduling ticks and halt until work arrives. Kernel syscalls execute on their calling CPU; filesystem services can run outside the entry gate. Most ordinary kernel entry still serializes. |
| Devices | Drivers publish bounded inventory records after initialization. The virtual `/dev` mount routes usable null/zero/random operations, browsable device/CPU directories and read-only metadata without granting raw hardware access. |

The local Linux reference at `6628e7d4c` was inspected read-only:
`drivers/acpi/osl.c:acpi_os_get_root_pointer` prefers a supplied root before legacy
lookup; `arch/x86/kernel/smpboot.c:common_cpu_up/do_boot_cpu` separates per-CPU setup,
trampoline parameters and wakeup. No Linux implementation was copied.

## Red evidence from this follow-up

- ACPI host cases rejected a valid RSDP before root parsing was implemented.
- `cargo test --lib` failed because valid boot ACPI tags produced `acpi_root=None`.
  A subsequent run exposed short required tag bodies accepted as valid information.
- The strengthened `e2e/smp.py --cpus 1` failed on the parent image with
  `CPU discovery ignored bootloader ACPI handoff`.
- The new Ring 3 device probe failed on that parent image: `/dev/null` returned
  `None` instead of EOF (`Some(0)`). devfs unit I/O failed for the same missing behavior.
- Registry host cases failed before registration/rendering was implemented.
- The FAT alias test failed: `/DEV./ZERO.` escaped the virtual mount classification.
- Repeated keyboard publication failed with `Duplicate` before one-time publication.

Initial boot/device red logs are under `target/smp-boot-audit/`; the directory
and shell-boundary regressions are under `target/dev-tree-directory/`. No existing timeout,
allocation count, expected error or assertion was relaxed.

The user also requested that the inventory be traversable and each CPU visible:

- The new Ring 3 directory regression failed on the first version with
  `device inventory must be browsable`: `/dev/devices` was an inventory file.
- Short-name tests exposed invalid device names and a collision between `cpu10`
  and prefix `cpu1` plus instance `0`; registration now validates canonical names.
- `/dev/devices` is now a directory. The aggregate text moved to its `info` file;
  `/dev/cpu/N/info` and `/dev/devices/cpuN/info` identify the same registered CPU.
- Directory tests require every expected CPU entry and flag, ordinary-process
  access, out-of-range rejection, matching aliases and read-only metadata.

- The 32-CPU desktop check then failed because Forge's 32-entry `ls` stopped at
  CPU 24. Its 1 KiB `cat` also truncated the aggregate inventory before the
  framebuffer record. Both failures were reproduced before raising the bounded
  userspace capacities to 128 entries and 8 KiB. No kernel listing assertion or
  timeout was weakened; the UI check now requires the last CPU and directory flags.

## Executed checks

Checks run on this stacked implementation (not inherited parent results):

- `cargo build`, `cargo build --features grub`, release GRUB build and
  `cargo test --no-run` passed as build evidence. `tools/build-grub-disk.sh`
  rebuilt all applications and the disposable image. `e2e/build_profile.sh`
  verified the release kernel in its generated disk.
- `cargo test --lib`: **96 cases** passed, including ACPI/root/tag validation,
  registry duplicate/capacity handling, devfs boundaries/aliases and idempotent
  driver publication. Standalone host runs passed **4 ACPI** and **3 registry**
  cases; these are also included in the kernel count.
- Nine selected kernel integration executables passed **16 cases**: `basic_boot`,
  `display_mapping`, `gfx`, `keyboard_timer`, `should_panic`, `stack_overflow`,
  `stdio`, `threads`, `userspace`. ATA passed **3 cases**, FAT32 **2**, release heap
  **5**. Writable fixtures were tested sequentially.
- Fresh `e2e/smp.py --cpus 1 2 8 16 32 --stress` passed startup acknowledgements,
  unique per-CPU GDT/TSS/control-register assertions, native CPU/syscall progress,
  noncooperative timer preemption on the last CPU, synchronization, thread/process
  lifecycle, faults, exhaustion and allocation/frame recovery. The real `/dev`
  probe passed from PID 0 and an ordinary child, including FAT backing-file
  shadowing, aliases, immutable nodes, buffer guards and unrelated-file I/O.
- Complete **16- and 32-CPU desktop suites** and the **16-CPU glass suite** passed commands/file results,
  focus/drag/close, shell reaping, overlapping/background damage and DE isolation.
  Forge ran `ls /dev`, `ls /dev/devices`, `ls /dev/cpu`, individual CPU info
  and aggregate inventory reads. The 32-CPU run required CPU 31 in both listings.
  The [16-CPU directory screenshot](screenshots/desktop-cpu-tree.png) shows the
  numbered CPU directories and CPU 0's BSP info; the
  [inventory screenshot](screenshots/desktop-device-tree.png) shows AP identities
  and initialized framebuffer, serial, keyboard, mouse, root disk and Ethernet drivers.
  It supplements the automated assertions.
- Two-terminal latency passed on **1/8/16/32 CPUs**. Desktop CPU/input-under-load
  budgets passed on **1 and 16 CPUs**. Measurements below are isolated QEMU/TCG
  runs with the standard 1 GiB platform; existing budgets are unchanged.
- Sixteen-CPU network checks passed the Ring 3 ownership/buffer/UDP/TCP probe,
  absent NIC, **48 packet-verified replies**, unreachable-peer timeout, HTTP/CURL,
  verified TLS including the public HTTPS case, missing entropy, invalid clock
  and the network throughput budget.
- **35 Python tooling cases**, all **4 SDK ownership compile/compile-fail probes**,
  documentation checks and `git diff --check` passed.

| CPUs | First / second launch (ms) | Worst pointer (ms) | Worst Enter response (ms) |
| --- | --- | --- | --- |
| 1 | 322.9 / 357.5 | 19.2 | 15.8 |
| 8 | 323.5 / 323.9 | 17.7 | 5.7 |
| 16 | 336.7 / 350.4 | 20.5 | 5.6 |
| 32 | 326.2 / 337.9 | 31.1 | 5.6 |

Launch includes the 300 ms click sequence. Both independent responses also met
the final-glyph framebuffer assertion within 300 ms. Per-CPU data remains under
`target/desktop-parallel/`; all follow-up logs are under `target/dev-tree-directory/`.
The installed kernel ELF and complete Forge/ZTHREADS binaries exactly matched
the final rebuilt files (`artifact-match.log`). The tested disk SHA-256 is `eee01b67369e5baa30f78e98d4bb0aad0e78f802724b69d118be87da682b6d45`.

The unchanged debug `cargo test --test heap_allocation` was rerun and timed out
at 60 seconds in the 67,108,864-allocation case, after the first four cases passed.
The earlier baseline reproduction belongs to the parent work; this follow-up
reproduces the same failure and does not claim that suite is green.


## Remaining limits

The device namespace is described in [devices](devices.md). It is not a full VFS,
POSIX descriptor API, ACPI AML interpreter or complete physical device enumeration.
Raw hardware endpoints, hotplug, I/O APIC routing, kernel worker pools, per-CPU
run queues and fully parallel/preemptible kernel entry remain future work.
The shared ready queue/entry gate still limits kernel throughput.

The debug heap timeout above remains an explicit failing required check. Actual
QEMU boot exercised the supplied ACPI 1/RSDT path; extended/XSDT selection is
covered by parser tests rather than an ACPI 2-only firmware boot. Manual host GUI input
and physical hardware remain separate, unverified acceptance criteria.
