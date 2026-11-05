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
  devices/
    info             read-only complete inventory text
    cpu0/info        one initialized CPU's identity and BSP/AP role
    fb0/info         one initialized driver's public properties
    ...
  cpu/
    0/info           alias of devices/cpu0/info
    1/info
    ...              one directory per online logical CPU
```

In Forge:

```text
ls /dev
ls /dev/devices
ls /dev/cpu
cat /dev/cpu/0/info
cat /dev/devices/info
write /dev/null discarded
```

## Ownership and discovery

[The core inventory](../src/devices/mod.rs) owns 64 fixed registration slots.
[Driver records](../src/devices/registry.rs) contain static names, discovery source,
instance identity, driver name and public scalar properties. Registration validates
unique case-folded 8.3 directory identities, including collisions between a prefix
and a numeric suffix; names cannot silently truncate or alias another device. They never contain
user buffers or expose kernel virtual addresses. Successful initialization publishes
records; an unavailable NIC or failed mouse initialization creates no hardware
record. Repeated keyboard publication is idempotent. Registration is boot-only;
there is no hotplug or user registration operation.

The text at `/dev/devices/info` groups records under `acpi/`, `boot/`, `legacy/`, and `pci/`:

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
| `/dev/devices/info` | Returns the inventory prefix fitting the buffer | Rejected |
| `/dev/devices/NAME/info` | One registered device's public properties | Rejected |
| `/dev/cpu/N/info` | The same CPU record as `/dev/devices/cpuN/info` | Rejected |

Supported files return zero for zero-length reads; directory reads fail. Inventory reads restart at the beginning on every
call; the current path API has no retained file offset or open handle. The
inventory is stable after boot. `ls /dev/devices` enumerates registered devices as
directories; `ls /dev/cpu` lists exactly the online logical indices `0..N-1`. Each
node directory contains `INFO`. Forge requests up to 128 directory entries, enough
for the complete bounded registry plus its inventory file. Its 8 KiB `cat` buffer
also covers the aggregate inventory at 32 CPUs. The former `/dev/devices` inventory file has moved
to `/dev/devices/info` so that the requested path is now traversable. These endpoints are accessible to ordinary
processes and grant no display, input, storage, CPU-control or network privileges.
Listing metadata is copied out before allocation; individual records are copied
before formatting, so no borrowed registry data survives unlocking.
Hardware control continues through existing typed interfaces and access rules.
There are no raw disk, port, MSR or framebuffer-write endpoints in this step.

Unknown paths under `/dev`, directory creation there, and attempts to spawn a
device as an executable fail. They never fall through to FAT creation/loading.
Case, repeated slash, `.` and FAT empty-extension aliases (`/DEV./ZERO.`) resolve
to the same mount; parent traversal is rejected. The root listing contains one
`DEV/` even if a stale FAT directory has that name. The virtual mount shadows it
without altering the backing contents. Listings retain the existing 8.3/12-byte
record format; directory entries use `0x10`, while endpoints and info files use `0x20`. There
are no new syscall numbers or Unix device numbers/permission bits.

Inventory locking is independent of scheduler and filesystem locks. Registration
runs during boot; no IRQ takes the inventory lock. Rendering the inventory holds
it only for bounded formatting into owned kernel storage, without allocation,
hardware I/O or scheduling.

## Verification

[Boot and device validation](smp-boot-audit-validation.md) records red failures,
executed checks and limits. Rebuild before tests:

```bash
./tools/build-grub-disk.sh target/dev-tree-final.img
RUSTOS_TEST_DISK=target/dev-tree-final.img python3 -B e2e/smp.py --cpus 1 2 8 16 32 --stress
RUSTOS_TEST_DISK=target/dev-tree-final.img RUSTOS_CPUS=16 python3 -B e2e/desktop.py
cargo test --lib
```

The Ring 3 probe checks real reads/writes and guards, PID 0 and ordinary callers,
CPU inventory counts, aliases, immutable paths, root merging and disk-file isolation.
The test plants a conflicting `/DEV/NULL` in its disposable FAT image and checks
that device writes leave those backing bytes unchanged. Host/kernel registry tests
exercise duplicate/capacity failures and missing hardware records. The desktop
suite executes `ls /dev`, `ls /dev/devices`, `ls /dev/cpu` and both aggregate and
individual `info` reads in Forge. It asserts the last CPU appears, including CPU 31
on a 32-CPU VM, and captures the tested result. Linux normally exposes CPU topology
under `/sys/devices/system/cpu`; this native `/dev/cpu/N/info` layout provides
read-only metadata, without Linux MSR/CPU-control device semantics.
