# Program arguments and focused validation

This records the initial argument implementation. The subsequent
[capability expansion](capabilities-validation.md) supersedes its fixed-block
SDK limits; syscall 29 remains supported for legacy callers.

Implemented on 2026-11-05 over the existing file-command work. The kernel copies
validated arguments into the child's private initial stack; Forge alone tokenizes
shell commands. Portal retains terminal responsibilities. See the
[wire/entry ABI](syscalls.md#program-arguments-and-entry).

## Using arguments

Existing `fn main()` programs remain supported. Applications can opt into:

```rust
use userlib::println;

fn main(argc: usize, argv: &[&str]) {
    println!("argc={}", argc);
    for (index, argument) in argv.iter().enumerate() {
        println!("argv[{}]={}", index, argument);
    }
}
```

Any application can instead use `userlib::args()` (an allocation-free iterator),
`userlib::args::arg(index)`, or raw `userlib::argc()` and `userlib::argv()`.
The raw vector is `*const *const u8` with a NULL final entry; dereferencing it
requires unsafe. The SDK's safe strings must remain immutable.

Spawn with `userlib::sys::spawn_with_args("/BIN/HELLO.BIN", &["alpha", "two words", ""])`.
The wrapper supplies argv[0] as the executable path. Connected spawning uses
`userlib::process::Child::spawn_with_args` with the same argument convention.
Forge supports `./hello alpha "two words" ""` and explicit executable paths.
The default SDK now uses syscall 30 with up to 4096 arguments including argv[0],
4096 environment entries and 128 KiB of combined NUL-inclusive strings. The old
32-argument/2032-byte limits apply only to legacy syscall 29. See the
[current ABI](syscalls.md#program-arguments-and-entry); exec/fork and Linux binary
compatibility remain unsupported.

## Observed red results

- The first Ring 3 argument-bearing spawn failed: syscall 29 was unknown and
  returned `SYSCALL_ERROR`, triggering `argument-bearing spawn must be supported`.
- Argument codec cases failed before encoding/validation existed: round trip and
  exact byte/count limits returned `InvalidArguments` for valid input.
- The private-stack layout test failed before stack construction existed.
- The shell tokenizer initially rejected a valid command with quoted spaces,
  empty strings, single quotes and backslash escapes.

## Checks run for this change

Only tests for argument formatting, startup memory/registers, process lifecycle,
connected spawning and the shell launch path were selected. The earlier file,
network, editor and heap suites were not rerun for argument support.

| Command | Result and coverage |
| --- | --- |
| `cargo +nightly-2025-01-30 test --offline --manifest-path /path/to/self_rust_os/crates/process-abi/Cargo.toml --target x86_64-unknown-linux-gnu` from `/tmp` | 4 passed: exact limits, empty/Unicode strings, malformed records, aligned startup stack, private pointers and NULL sentinels |
| `rustc --edition=2021 --test user_programs/forge/src/command.rs -o /tmp/rustos-command-tests` then `/tmp/rustos-command-tests` | 2 passed: quotes, escapes, empty arguments, whitespace, length/count limits and malformed commands |
| `cargo test --test threads --test stdio --test display_mapping` | 4 integration cases passed; checks actual saved RDI/RSI/RSP, private child argument bytes, preserved parent page, repeated frame/heap recovery, unchanged worker closure entry, connected endpoint cleanup and mapping guards |
| `RUSTOS_TEST_DISK=target/arguments.img python3 -B e2e/arguments.py` | Passed: real Ring 3 raw/typed/legacy/connected spawning, safe two-argument main and raw argv, empty/Unicode strings, invalid paths/records/pointers/flags, exact count/byte limits, parent-buffer mutation isolation, repeated creation/reaping and worker argument access |
| `RUSTOS_TEST_DISK=target/arguments.img RUSTOS_CPUS=8 python3 -B e2e/arguments.py` | The same Ring 3 argument assertions passed on 8 CPUs |
| `RUSTOS_TEST_DISK=target/arguments.img RUSTOS_DESKTOP_CASE=arguments python3 -B e2e/desktop.py` | Passed: Forge rejects incomplete quoting, delivers four exact argv entries to Hello, Hello reaches its UI loop, and the original shell remains responsive after child close |

All applications and their tracked binary/metadata pairs were regenerated with
`tools/build-user.sh` through `tools/build-grub-disk.sh target/arguments.img`.
The default kernel and GRUB debug/release builds passed. Build checks establish
compatibility; the Ring 3 tests establish actual delivery. Every QEMU test boots a
private writable copy of the disk. The argument-only mode selects the argument
probe without running its unrelated file-I/O cases.

## Evidence and remaining limits

[The screenshot](screenshots/desktop-arguments.png) captures Hello launched from
Forge after the argument assertions. The serial assertions establish exact argc
and argv values; appearance alone cannot prove argument delivery. The focused
probe retains diagnostics under `target/arguments/serial.log`.

Manual host GUI input remains unverified; QMP injected the guest input. Initial
processes spawned directly by the kernel have argc zero; legacy syscall 8 supplies
one resolved-path argument. Argument strings are UTF-8, not arbitrary Unix bytes.
This native extern-C entry uses RDI/RSI (and now RDX for envp) and an aligned initial stack; it is not the
Linux ELF entry-stack ABI. Ordinary spawned children retain their existing separate
console behavior; this change does not add inherited terminal streams or pipelines.

The earlier debug heap stress timeout remains recorded in
[file-command validation](file-commands-validation.md#screenshots-and-limits).
It was reproduced on unchanged main during the preceding work and was not rerun
here, because argument staging cleanup is exercised by the focused lifecycle test.
