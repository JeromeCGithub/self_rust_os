# self_rust_os
A self made simple OS in Rust.

This is an experimental repository to learn how to make an OS in Rust.

This project is based on the blog post series [Writing an OS in Rust](https://os.phil-opp.com/). I recommend you to read it if you want to learn how to make an OS in Rust.

I'm following the blog post of Philipp Oppermann, then trying to enhance the OS with more features that make me learn more about OS development.

## Features

- [x] VGA text buffer
- [x] Interrupts
- [x] Memory management
- [x] Multitasking (async executor)
- [x] User mode (Ring 3 execution with syscall interface)
- [x] In-memory file system (`ramfs`)
- [x] ATA disk driver (PIO block device)
- [x] FAT32 filesystem (format, create/read/write/delete files)

## Requirements

- [QEMU](https://www.qemu.org/)
- [Rust](https://www.rust-lang.org/) (nightly, see `rust-toolchain`)

You will need to install these dependencies:

```bash
$ cargo install bootimage
$ rustup component add llvm-tools-preview
```

## How to run

```bash
$ cargo run
```

The kernel boots, initializes all subsystems, loads the embedded user-mode binary,
switches the CPU to Ring 3, and executes the user program. The user program prints
messages to the VGA text buffer via syscalls and then exits cleanly back to the
kernel.

## How to run the tests

```bash
$ cargo test
```

The ATA integration test needs a test disk image attached (it is attached
automatically via `test-args` in `Cargo.toml`):

```bash
$ ./e2e/ata.sh   # runs the ATA driver E2E in QEMU and prints metrics
```

Test fixtures live in `tests/fixtures/` (`disk-ata.img` is used by the ATA
tests as the primary slave; the boot disk stays the primary master and is
never written by tests).

## Storage

### ATA driver (`src/ata.rs`)

Polling PIO-mode driver for IDE disks (LBA28, up to 128 GiB):

```rust
use self_rust_os::ata::{AtaDisk, AtaDrive};

let mut disk = AtaDisk::primary(AtaDrive::Slave);
let info = disk.identify()?.expect("no disk");
let mut sector = [0u8; 512];
disk.read_sector(42, &mut sector)?;
disk.write_sector(42, &sector)?;
```

- `identify()` returns the model/serial/sector count, or `None` when no
  device is attached.
- Every operation polls the status register with a bounded timeout, so a
  missing device fails with `AtaError::Timeout` instead of hanging the kernel.
- Disk interrupts are not used; cache flushes are issued after every write.

### FAT32 filesystem (`src/fat32.rs`)

Self-contained FAT32 implementation generic over a `BlockDevice` trait
(so it is unit-tested against a RAM mock and runs on the real ATA disk):

```rust
use self_rust_os::ata::{AtaDisk, AtaDrive};
use self_rust_os::fat32::Fat32;

let disk = AtaDisk::secondary(AtaDrive::Master);
let mut fs = Fat32::format(disk, 32768, b"RUSTOS     ")?;
fs.create_file("HELLO.TXT")?;
fs.write_file("HELLO.TXT", b"Hello, disk!")?;

let mut buf = [0u8; 64];
let n = fs.read_file("HELLO.TXT", &mut buf)?;
```

- `format` / `open` (BPB parsing + validation), `create_file`,
  `write_file`, `read_file`, `delete_file`, `list_dir`
- 8.3 filenames, root directory, cluster-chain allocation
- E2E: `./e2e/fat32.sh` (uses `tests/fixtures/disk-fat32.img` as secondary master)

## User Space

The OS supports executing user-mode binaries in Ring 3. A user program communicates
with the kernel through a syscall interface triggered by the `int 0x80` software
interrupt.

### Syscall ABI

| Register | Purpose        |
|----------|----------------|
| `rax`    | syscall number |
| `rdi`    | argument 1     |
| `rsi`    | argument 2     |
| `rdx`    | argument 3     |

The return value is placed in `rax`.

#### Available syscalls

| Number | Name        | Arguments                                            | Description                                |
|--------|-------------|------------------------------------------------------|--------------------------------------------|
| `0`    | `sys_exit`  | `rdi` = exit code                                    | Terminates the user process.               |
| `1`    | `sys_write` | `rdi` = buffer ptr, `rsi` = length                   | Writes a buffer to the VGA text display.   |
| `2`    | `sys_open`  | `rdi` = path ptr, `rsi` = path len, `rdx` = flags    | Opens (or creates) a file, returns a fd.   |
| `3`    | `sys_close` | `rdi` = fd                                           | Closes an open file descriptor.            |
| `4`    | `sys_read`  | `rdi` = fd, `rsi` = buffer ptr, `rdx` = length       | Reads bytes from an open file.             |
| `5`    | `sys_fwrite`| `rdi` = fd, `rsi` = buffer ptr, `rdx` = length       | Writes bytes to an open file.              |

`sys_open` flags: `O_READ = 0x01`, `O_WRITE = 0x02`, `O_CREATE = 0x04`,
`O_TRUNC = 0x08`, `O_APPEND = 0x10`. Unknown syscall numbers return
`u64::MAX - 1`.

### Building the user program

A sample user program lives in `user_programs/hello/`. It is a minimal `no_std`
Rust binary that prints a greeting message via `sys_write` and terminates via
`sys_exit`.

To rebuild the user program after making changes:

```bash
$ cd user_programs/hello
$ ./build.sh
```

This compiles the Rust binary for the custom `x86_64-user-program` target and
converts the resulting ELF to a flat binary (`hello.bin`) using `llvm-objcopy`.
The flat binary is linked at virtual address `0x400000` with `_start` as the
entry point.

The kernel embeds `hello.bin` via `include_bytes!` at compile time, so after
rebuilding the user program you must also rebuild the kernel:

```bash
$ cargo run
```

### Writing your own user program

1. Create a new `no_std`, `no_main` Rust crate (you can copy `user_programs/hello/`
   as a template).
2. Use the `int 0x80` syscall interface to interact with the kernel. Example:

   ```rust
   unsafe fn syscall(num: u64, arg1: u64, arg2: u64) -> u64 {
       let result: u64;
       core::arch::asm!(
           "int 0x80",
           inlateout("rax") num => result,
           in("rdi") arg1,
           in("rsi") arg2,
           lateout("rcx") _,
           lateout("r11") _,
           options(nostack),
       );
       result
   }
   ```

3. Mark your entry point with `#[no_mangle]` and `#[link_section = ".text.start"]`
   so the linker places it at the base address of the binary.
4. Build with the provided target JSON and linker script, then convert to a flat
   binary with `llvm-objcopy -O binary`.
5. Embed the binary in the kernel with `include_bytes!` and pass it to
   `userspace::process::run()`.

## File system (`ramfs`)

The kernel embeds a tiny in-memory filesystem. It has a flat namespace
(every file lives at the root, e.g. `/hello.txt`), supports up to 32 files
of 64 KiB each, and tracks up to 32 open file descriptors with per-fd
cursors. Paths must be absolute, use only `[A-Za-z0-9._-]` components, and
be at most 64 bytes long.

The filesystem is volatile: everything is lost on reboot. At boot the
kernel seeds it with `/welcome.txt`. The sample user program
(`user_programs/hello/`) demonstrates the file syscalls end to end: it
creates `/hello.txt`, writes a greeting, reads it back, and prints it.

## Contributing

If like me you want to learn more about OS development in rust, feel free to contribute to this project. You can open an issue or a pull request. I will be happy to discuss with you about this project and its implementation.

## License  

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.

This project is also licensed under the Apache 2.0 License - see the [LICENSE-APACHE](LICENSE-APACHE) file for details.