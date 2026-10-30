# Development workflow

## Environment and commands

Run commands from the repository root. The pinned [toolchain](../rust-toolchain.toml)
is `nightly-2025-01-30`, with `rust-src` and `llvm-tools-preview`. Keep it pinned:
new compiler releases can change naked assembly, target JSON, and `build-std` behavior.
The root [.cargo config](../.cargo/config.toml) selects the custom bare-metal target
and builds `core`, `compiler_builtins`, and `alloc`; host `cargo test` semantics do not apply.

Host prerequisites are Rust/rustup, `cargo-bootimage` (`cargo install bootimage`),
QEMU x86, GRUB BIOS tools (`grub-mkimage` and `/usr/lib/grub/i386-pc/boot.img`), mtools
(`mformat`, `mcopy`, `mmd`), Python 3, Pillow for screenshots, and standard GNU shell tools.
Package names vary; Debian/Ubuntu commonly provide `qemu-system-x86`, `grub-pc-bin`,
`mtools`, and `python3-pil`. Install prerequisites through the user's normal environment;
do not change the OS toolchain merely to work around a missing host utility.

| Command | Meaning |
| --- | --- |
| `cargo build` | Compile the default kernel boot path; does not prove desktop boot |
| `cargo build --features grub` | Compile the GRUB kernel |
| `cargo build --release --features grub` | Compile optimized GRUB kernel |
| `./tools/build-user.sh` | Build every application; generate flat binaries and section metadata |
| `./tools/build-user.sh portal` | Build one application and regenerate its artifacts |
| `./tools/build-grub-disk.sh` | Build applications/release kernel and overwrite `target/grub-disk.img` |
| `cargo run` | Build the desktop disk and start graphical QEMU through the runner |
| `RUSTOS_DISPLAY=none cargo run` | Same boot path without GUI; runs until terminated |
| `python3 e2e/desktop.py` | Runtime desktop regression checks after a fresh disk build |
| `python3 -B tools/check_docs.py` | Check required guides and local inline Markdown links/anchors |
| `python3 -B -m unittest discover -s tools/tests -p 'test_*.py'` | Test the documentation checker |

Do not overwrite a disk attached to a running QEMU. Builders and fixture tests write
image files. Stop the VM first and use disposable paths. Do not treat files written
inside the development disk as durable: the next build replaces the disk.

## Required red → green → refactor sequence

1. **Understand:** read [root instructions](../AGENTS.md), scoped instructions, source,
   callers, and [architecture](architecture.md). Establish acceptance criteria, baseline,
   and affected trust boundaries. Identify an observable result for each criterion.
2. **Red:** add the smallest behavioral regression test, run it, and save the command
   and expected failure. If the environment prevents it from executing, resolve that
   or report the blocker; a missing tool is not evidence of the requested behavior.
3. **Green:** implement only what the test requires. For ABI work, update dispatcher,
   SDK, application call sites, and [ABI documentation](syscalls.md) together. Rebuild
   all dependent artifacts before running the test against a fresh disk.
4. **Refactor:** improve names, invariants, interfaces and duplication while the tests
   remain green. Keep unrelated cleanup out of the change.
5. **Verify all affected paths:** follow the [test matrix](testing.md). Add negative,
   boundary, and lifecycle tests, run applicable complete suites, inspect visual output
   when relevant, and review the final diff. Do not rely on generated files from before
   the source change or on test results from an earlier commit with different behavior.
6. **Report:** provide the observed red result, green checks, tested behaviors, baseline
   failures, and unverified/manual criteria. Never say “all tests pass” when only builds
   or a subset ran. Never hide a failure behind a screenshot or a `|| true` pipeline.

For prose changes, define the expected documentation entry points/links and run the
checker before and after. For checker/tool changes, write executable regression tests
first. Runtime boot tests are required when documented runnable behavior changes in
code; they are not required for a wording-only fix.

## Building and changing userspace

Use `userlib` rather than hand-written syscall assembly. Programs implement
`fn main()`; [the runtime](../user_programs/userlib/src/rt.rs) supplies `_start` and
the panic handler. Keep linker scripts/custom targets aligned with kernel loading.

The unified builder converts ELF to flat `.bin`, materializes `.bss`, and writes
`*_meta.rs`. These artifacts are tracked and must match source. Never edit them
manually. The boot disk contains binaries loaded at runtime; the kernel no longer
embeds `hello.bin`. Adding an app requires a separate crate, compatible target/linker
layout, `userlib`, and a rebuilt disk. `/BIN` discovery provides its launcher.

## Review and completion evidence

A PR should explain the trigger/problem, changed observable behavior, privilege
boundary effects, and validation. For unsafe/assembly changes include the invariant
and why it remains valid. For ABI changes include compatibility and migration details.
Every PR must include concrete evidence that the change works, with screenshots
whenever practical. For graphics/UI/input or visible boot changes, capture a screenshot
from the tested build and explain what it demonstrates, alongside automated assertions.
For resource ownership include failure/cleanup evidence. If a screenshot would not
establish correctness (for example, a documentation-only or internal algorithm change),
or cannot be captured, state why and provide actual test output, serial logs, traces,
or a reproducible before/after example. Do not fabricate images or reuse unrelated
artifacts. Link or attach evidence in the PR instead of claiming hypothetical coverage.

Before requesting review, run the applicable tests, `git diff --check`, inspect
`git diff`/`git status`, and verify the branch base. Do not commit build caches, logs,
temporary sockets, unrelated fixture changes, or disk images. Preserve user work.
Disclose baseline runtime failures independently of passing documentation/tool tests.


## Writing an application

Application source uses ordinary Rust functions:

```rust
use userlib::println;

fn main() {
    println!("Hello from RustOS");
}
```

Each application manifest selects the SDK's shared executable root:

```toml
[[bin]]
name = "myapp"
path = "../userlib/src/application.rs"
```

Keep application code in `src/main.rs`. The shared root includes that file,
supplies the target's `no_std`/`no_main` attributes, and bridges `main` to the
existing runtime. Returning from `main` exits with status zero; use
`userlib::sys::exit(code)` for an explicit status. Panics report through userlib
and exit with status one. The supported entry signature is `fn main()`, with no
arguments or return value. Use regular comments in the included application root;
crate-level attributes and inner documentation belong in the shared root.

This is a Cargo convention for the current custom target, not a port of Rust's
standard library. `core` and `userlib` are available; APIs such as `std::fs`,
threads and networking are not. Dependencies must support `no_std` and the pinned
target. The OS still loads fixed-address flat binaries rather than Linux ELF files.
The shared root does not change syscall numbers, linker layout, or privileges.
