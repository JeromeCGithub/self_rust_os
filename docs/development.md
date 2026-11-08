# Development workflow

## Environment and commands

Run commands from the repository root. The pinned [toolchain](../rust-toolchain.toml)
is `nightly-2025-01-30`, with `rust-src` and `llvm-tools-preview`. Keep it pinned:
new compiler releases can change naked assembly, target JSON, and `build-std` behavior.
Active desktop/E2E launchers use the shared QEMU platform profile with 1 GiB RAM;
bootimage tests also request 1 GiB. Initial user stacks are 4 MiB and the kernel heap is 64 MiB. The SDK heap grows
with `brk` instead of reserving fixed BSS storage; physical pages are mapped eagerly.
User targets enable SSE2 and local-exec compiler TLS, with per-thread kernel state
preservation. Rebuilding all apps and the disk is required after ABI/layout changes.
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
| `./tools/build-user.sh` | Build every application; generate compact ELF images and permission metadata |
| `./tools/build-user.sh portal` | Build one application and regenerate its artifacts |
| `./tools/build-grub-disk.sh` | Build applications/release kernel and overwrite `target/grub-disk.img` |
| `cargo run` | Build the desktop disk and start graphical QEMU with 8 CPUs through the runner |
| `RUSTOS_CPUS=32 cargo run` | Select 1–32 CPUs; invalid counts are rejected |
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

The unified builder strips ELF debug/symbol data into `.bin`, preserves PT_LOAD
file/memory sizes (BSS is zeroed in RAM), and writes `*_meta.rs`. Metadata uses
page-aligned load permissions, including GOT sections sharing a page. These artifacts are tracked and must match source. Never edit them
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
and exit with status one. Supported signatures are `fn main()` and
`fn main(argc: usize, argv: &[&str])`, both returning `()`. The latter receives
argv[0] followed by the supplied arguments. Any program can also use
`userlib::args()` or raw `userlib::argc()` / `userlib::argv()` access. See
[program arguments](syscalls.md#program-arguments-and-entry).
Use regular comments in the included application root;
crate-level attributes and inner documentation belong in the shared root.

This is a Cargo convention for the current custom target, not a port of Rust's
standard library. `core` and `userlib` are available; APIs such as `std::fs` and `std::thread` are not. Networking is provided by
`userlib::net`, `userlib::http` and the optional `userlib::tls` feature. Dependencies must support `no_std` and the pinned
target. Native threads are available through `userlib` with `features = ["threads"]`;
see [the threading guide](threading.md). The OS loads fixed-address static ELF images and supports legacy flat binaries.
Using the ELF container does not provide Linux ABI compatibility or dynamic linking.
The shared root does not change syscall numbers, linker layout, or privileges.

## Native Initator boot profile

The default desktop still boots its compositor as PID 0. To build the separate
headless service profile into an explicit disposable image:

```bash
RUSTOS_BOOT_PROFILE=native-init ./tools/build-grub-disk.sh target/init-m1.img
RUSTOS_TEST_DISK=target/init-m1.img RUSTOS_CPUS=8 python3 -B e2e/init.py --case two-services
```

The profile enables kernel features `grub,native-init`; invalid profile values
fail before builds or image writes. Initator (`/BIN/INITATOR.SVC`), Glow and Flicker
are packaged with the existing `.service` marker and stay out of desktop icons.
`zinit` is a `.test-only` probe installed only into a runner's private image copy.
The runner's other cases cover faulting services, orphans, ABI rejection,
reclamation, allocation failure, missing/bad initial images and fatal PID 1.
Use `python3 -B e2e/init.py --help` for accepted case names. Never rebuild an
attached image; each runner boots its own writable copy.

M1 uses compiled definitions and coarse native tick deadlines. It is a trusted
service experiment, not a secure multiuser system. System/user units, service IPC,
configuration and graphical-session supervision follow in separate milestone PRs.
See [current evidence and limitations](initator-progress.md).
