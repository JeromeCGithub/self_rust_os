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
- [ ] File system

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

| Number | Name        | Arguments                          | Description                                |
|--------|-------------|------------------------------------|--------------------------------------------|
| `0`    | `sys_exit`  | `rdi` = exit code                  | Terminates the user process.               |
| `1`    | `sys_write` | `rdi` = buffer ptr, `rsi` = length | Writes a buffer to the VGA text display.   |

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

## Contributing

If like me you want to learn more about OS development in rust, feel free to contribute to this project. You can open an issue or a pull request. I will be happy to discuss with you about this project and its implementation.

## License  

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.

This project is also licensed under the Apache 2.0 License - see the [LICENSE-APACHE](LICENSE-APACHE) file for details.