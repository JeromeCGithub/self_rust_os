# Device inventory and `/dev`

The kernel mounts a virtual `/dev` namespace above the FAT32 root. It is created
in memory at boot, without a disk directory or user-created device nodes. Existing
`userlib::sys::{ls,read_file,write_file}` calls reach it through the same validated
syscall boundary as ordinary files. This is a first native device interface, not
Linux ABI or POSIX file-descriptor compatibility.

```text
/dev/
  null       EOF on read; discard writes
  zero       zero-fill reads; discard writes
  random     bounded hardware entropy reads
  devices    read-only tree of initialized driver instances
```

In Forge:

```text
ls /dev
cat /dev/devices
write /dev/null discarded
```

## Ownership and discovery

[The core inventory](../src/devices/mod.rs) owns 64 fixed registration slots.
[Driver records](../src/devices/registry.rs) contain static names, discovery source,
instance identity, driver name and public scalar properties. They never contain
user buffers or expose kernel virtual addresses. Successful initialization publishes
records; an unavailable NIC or failed mouse initialization creates no hardware
record. Repeated keyboard publication is idempotent. Registration is boot-only;
there is no hotplug or user registration operation.

The inventory groups records under `acpi/`, `boot/`, `legacy/`, and `pci/`:

- ACPI MADT supplies CPU/APIC identities on the GRUB platform. Each CPU appears
  after its startup acknowledgement, with its dense index and BSP/AP role.
- Boot framebuffer geometry supplies `fb0`; this is an inventory record, not a
  new unrestricted framebuffer mapping.
- Legacy PC mechanisms initialize the serial, keyboard, mouse and root ATA device.
  The mouse record identifies the absolute or relative transport. The root disk
  record reports its partition LBA and sector count in explicit units.
- PCI enumeration initializes the supported e1000 NIC; it appears only on success.
- The software branch describes the built-in null/zero and entropy endpoints.
  The random endpoint can exist when RDRAND is unavailable; reads then fail.

This is the initialized-driver inventory, not an enumeration of every physical
or firmware device. ACPI AML/DSDT enumeration, a Flattened Device Tree parser,
I/O APIC routing, bus hotplug and loadable drivers remain future work. ACPI and
PCI discovery have distinct responsibilities; scheduling work is not stored in
the device tree. See [CPU boot and scheduling](threading.md).

## Device operations and boundaries

[devfs](../src/fs/devfs.rs) handles device semantics; the
[filesystem facade](../src/fs.rs) selects devfs or FAT before taking the disk
service lock. The dispatcher copies complete paths/buffers into owned kernel
storage and checks user mappings before access. No user reference survives a
lock, scheduling operation or device read.

| Path | Read | Write |
| --- | --- | --- |
| `/dev/null` | Returns zero bytes, preserving the buffer | Accepts and discards all supplied bytes |
| `/dev/zero` | Fills the requested buffer with zeros | Accepts and discards all supplied bytes |
| `/dev/random` | Up to 256 bytes per call; fails if hardware entropy is absent or bounded retries fail | Rejected |
| `/dev/devices` | Returns the inventory prefix fitting the buffer | Rejected |

Zero-length reads return zero. Inventory reads restart at the beginning on every
call; the current path API has no retained file offset or open handle. The
inventory is stable after boot. These endpoints are accessible to ordinary
processes and grant no display, input, storage, CPU-control or network privileges.
Hardware control continues through existing typed interfaces and access rules.
There are no raw disk, port, MSR or framebuffer-write endpoints in this step.

Unknown paths under `/dev`, directory creation there, and attempts to spawn a
device as an executable fail. They never fall through to FAT creation/loading.
Case, repeated slash, `.` and FAT empty-extension aliases (`/DEV./ZERO.`) resolve
to the same mount; parent traversal is rejected. The root listing contains one
`DEV/` even if a stale FAT directory has that name. The virtual mount shadows it
without altering the backing contents. Listings retain the existing 8.3/12-byte
record format; device entries use its ordinary non-directory attribute. There
are no new syscall numbers or Unix device numbers/permission bits.

Inventory locking is independent of scheduler and filesystem locks. Registration
runs during boot; no IRQ takes the inventory lock. Rendering the inventory holds
it only for bounded formatting into owned kernel storage, without allocation,
hardware I/O or scheduling.

## Verification

[Boot and device validation](smp-boot-audit-validation.md) records red failures,
executed checks and limits. Rebuild before tests:

```bash
./tools/build-grub-disk.sh target/smp-boot-audit.img
RUSTOS_TEST_DISK=target/smp-boot-audit.img python3 -B e2e/smp.py --cpus 1 2 8 16 32 --stress
RUSTOS_TEST_DISK=target/smp-boot-audit.img RUSTOS_CPUS=16 python3 -B e2e/desktop.py
cargo test --lib
```

The Ring 3 probe checks real reads/writes and guards, PID 0 and ordinary callers,
CPU inventory counts, aliases, immutable paths, root merging and disk-file isolation.
The test plants a conflicting `/DEV/NULL` in its disposable FAT image and checks
that device writes leave those backing bytes unchanged. Host/kernel registry tests
exercise duplicate/capacity failures and missing hardware records. The desktop
suite executes `ls /dev` and `cat /dev/devices` in Forge and captures the result.
