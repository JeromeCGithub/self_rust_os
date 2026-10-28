# self_rust_os

An experimental x86-64 operating system written from scratch in Rust for learning,
based on Philipp Oppermann's [Writing an OS in Rust](https://os.phil-opp.com/).
It is not a production OS or a Linux/POSIX-compatible system.

## Start here

- [Project documentation](docs/README.md): architecture, builds, testing and syscall ABI.
- [AI contribution rules](AGENTS.md): mandatory test-driven development and evidence
  before claiming completion. Read scoped instructions before editing a subsystem.
- [Kernel versus userspace](docs/architecture.md): `src/` is Ring 0, including
  `src/userspace/`; apps and `userlib` under `user_programs/` run in Ring 3.

## Current features

- GDT/TSS, interrupts, paging, heap, serial diagnostics, and preemptive user processes.
- ATA PIO storage, MBR partitions, FAT32 files/directories and file syscalls.
- GRUB/Multiboot2 desktop boot with a 1920×1080×32 framebuffer.
- Userspace compositor with an image wallpaper, discovered application icons,
  private application surfaces, window stacking, dragging, close buttons and focus.
- Portal graphical terminal, Forge text shell, Hello, Snake and CPU-bound Spin app.
- QEMU absolute pointer integration, with PS/2 fallback, plus keyboard input.

## Requirements and running

Use the pinned [Rust nightly](rust-toolchain.toml), including `rust-src` and
`llvm-tools-preview`. Host tools: QEMU x86, GRUB BIOS utilities, mtools, Python 3,
and Pillow for screenshot tests. Kernel tests also require `cargo-bootimage`:

```bash
cargo install bootimage
cargo run
```

See [development setup](docs/development.md) for details. On Debian/Ubuntu the system
packages commonly include `qemu-system-x86`, `grub-pc-bin`, `mtools`, and `python3-pil`.

`cargo run` builds every app, creates a GRUB/FAT32 disk at `target/grub-disk.img`, and
starts QEMU. **Rebuilding overwrites that development disk.** Stop any VM using it
before rebuilding. `RUSTOS_DISPLAY=none cargo run` boots without a graphical window.

Move the host pointer over the display; absolute integration avoids capture where
supported. **Ctrl+Alt+G** toggles QEMU input capture if needed. Click an icon to open
an app, click a window to focus it, drag the title bar, and click **x** to close it.
Up to six windows can be open; exited apps remain visible until closed.

Terminal commands: `help`, `echo`, `clear`, `pwd`, `ls [path]`, `cat /file.txt`,
`write /file.txt text`, `mkdir /directory`, `time`, and `exit`. Paths are rooted at `/`
and filenames use FAT32 8.3 conventions. Command and desktop policy lives in userspace.

## Building and testing

```bash
cargo build
cargo build --features grub
cargo build --release --features grub
./tools/build-user.sh
cargo test --no-run
```

These compile code; they do not prove runtime behavior. Follow the complete
[testing matrix](docs/testing.md) for the affected subsystem. Kernel tests use QEMU
and custom `#[test_case]` harnesses, not ordinary host tests. The full `cargo test`
command currently includes a nonterminating graphics demo and known baseline failures;
do not claim it is a green unattended gate.

```bash
# Focused kernel runtime checks
cargo test --test basic_boot --test gfx --test heap_allocation --test userspace
./e2e/ata.sh
./e2e/fat32.sh

# Current desktop runtime checks: build fresh disk first
./tools/build-grub-disk.sh
python3 e2e/desktop.py

# Documentation/tooling checks
python3 -B -m unittest discover -s tools/tests -p 'test_*.py'
python3 -B tools/check_docs.py
```

ATA/FAT32 checks mutate dedicated image fixtures. Do not replace them with host disks
or run concurrent writers against one fixture. Screenshots are in
[docs/screenshots](docs/screenshots).

To rebuild one app, use `./tools/build-user.sh portal` (substitute the crate name).
The builder regenerates both flat `.bin` files and `_meta.rs` layout metadata.
Programs use [userlib](user_programs/userlib/src/lib.rs) and implement `user_main`;
the SDK supplies `_start` and the panic handler. Rebuild the disk after an app change.

## Interfaces and limitations

The [current syscall ABI](docs/syscalls.md) uses `int 0x80`. It provides process,
keyboard, file-by-path, display/surface and focus operations. It does not implement
the old ramfs open/close API, Linux syscall numbers, or a POSIX descriptor model.
The root filesystem is now FAT32, and the kernel loads the WM from disk at runtime.

The desktop defaults to 1920×1080. Process frames/stacks are not fully reclaimed
on exit; repeated launches can exhaust memory. User-pointer validation and code
permissions also need further hardening. See [architecture](docs/architecture.md)
for boundaries and exceptions and [known test gaps](docs/testing.md#known-baseline-gaps)
for reproducible baseline issues. Do not confuse compile success with isolation,
cleanup, Ring 3 execution, or manual host GUI verification.

## Contributing

Use red → green → refactor, preserve the kernel/userspace boundary, and include
actual test evidence and limitations in every PR, with screenshots whenever practical. Questions and learning-oriented
contributions are welcome. See [development workflow](docs/development.md).

## License

Dual licensed under [MIT](LICENSE-MIT) and [Apache 2.0](LICENSE-APACHE).
