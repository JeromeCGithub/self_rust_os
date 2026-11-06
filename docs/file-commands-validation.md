# File commands and editor validation

This records the initial file-command implementation. Its original descriptor,
file-size and 8.3-only limits are superseded by the
[capability expansion](capabilities-validation.md); historical check results below
refer to the implementation tested at that step.

Implemented and tested on 2026-11-05, based on `main` commit `3277448`.
Commands and editor policy run in the Forge shell in Ring 3. Portal retains only
terminal input/output/rendering responsibilities. The kernel owns cwd, descriptor
lifetimes, FAT32 operations and syscall validation. See the
[command guide](terminal-shell.md#filesystem-commands-and-medit) and
[ABI contract](syscalls.md#regular-files-and-working-directories).

## Observed red results

- Fresh baseline image: the desktop interaction test reached `cd /demo` and Forge
  returned `unknown command: cd /demo`, failing the expected `cwd: /DEMO` assertion.
- The new Ring 3 probe installed into a private baseline image failed its first
  `chdir` assertion: syscall 27 was unknown and returned the native error value.
- FAT32 disk-full regression: replacing `KEEP.TXT` failed with `DiskFull` but reading
  it returned seven `42` bytes instead of `keep me`. The old implementation freed
  its original chain before allocating the replacement.
- Offset-read regression failed with `Err(BufferTooSmall)` instead of reading
  700 bytes across a cluster boundary. Four editor model cases also failed before
  modal editing was implemented.
- Further boundary checks reproduced empty `chdir` succeeding and `getcwd` accepting
  an overflowing advertised capacity. Both now return errors.
- The packaging regression initially installed `ZFILEIO.BIN` into normal `/BIN`.
  This also exposed the changed desktop launcher count in the HTTP suite. A
  `.test-only` marker now excludes the probe from ordinary disks while still
  rebuilding its binary/metadata. The file-I/O runner installs it only in its
  private image. Existing launcher expectations and positions are preserved.

## Checks run on this implementation

| Check | Result and useful coverage |
| --- | --- |
| `rustc --edition=2021 --test user_programs/forge/src/editor.rs -o /tmp/rustos-editor-tests` then `/tmp/rustos-editor-tests` | 4 passed: insertion, movement, deletion, backspace/newline, empty boundaries, dirty quit, force quit, save actions and bounded document/command storage |
| `cargo test --lib` | 87 passed, including FAT32 replacement rollback, offset reads and syscall validation |
| `cargo test --test basic_boot --test gfx --test userspace --test display_mapping --test should_panic --test stack_overflow --test keyboard_timer --test stdio --test threads` | Selected boot/display/process/stdio/native-thread integration checks passed (17 reported `[ok]` markers); `userspace` constants alone do not establish Ring 3 behavior |
| `./e2e/ata.sh`, `./e2e/fat32.sh` | Passed against explicit disposable fixtures; sector round trip and real FAT32 create/read/replace/delete coverage |
| `cargo test --release --test heap_allocation` | All 5 passed |
| `RUSTOS_TEST_DISK=target/file-commands.img RUSTOS_CPUS=N python3 -B e2e/file_io.py`, N = 1, 8, 32 | Passed: real Ring 3 descriptors, independent offsets, seek/gaps/EOF, permissions by open access mode, invalid user mappings and capacities, cwd isolation/inheritance, quota failure without disk mutation, repeated open/close and exit/kill cleanup; eight threads each receive every shared-file record exactly once |
| `RUSTOS_TEST_DISK=target/file-commands.img python3 -B e2e/desktop.py` | Passed: actual Portal/Forge cd/mkdir/write/cat, failed cd preserving cwd, editor save/reopen, dirty quit refusal, discard without changing the saved file, cat beyond 1 KiB, and existing focus/concurrency/drag/close checks |
| `RUSTOS_TEST_DISK=target/file-commands.img python3 -B e2e/http.py` | Passed: Portal curl, standalone CURL, Ring 3 DNS/HTTP and cleanup remain compatible with file-backed configuration and spawning |
| `RUSTOS_TEST_DISK=target/file-commands.img python3 -B e2e/network.py --probe` | Passed: existing Ring 3 user-buffer, ownership, cleanup, UDP/TCP/HTTP paths |
| `python3 -B -m unittest discover -s tools/tests -p 'test_*.py'` | 36 passed, including the new test-only packaging regression |

Build checks also passed: `cargo build`, `cargo build --features grub`,
`cargo build --release --features grub`, `cargo test --no-run`,
`./tools/build-user.sh`, and `./tools/build-grub-disk.sh target/file-commands.img`.
These are build evidence, not substitutes for the execution checks above. The disk
builder executes the updated app installer, and QEMU boots that resulting disk.
Tests use private writable copies, never a disk attached to another running VM.

The final sources were rebuilt before the applicable runtime checks. Later
packaging-only edits needed the host regression, image build/boot and desktop
compatibility checks; unrelated kernel suites were not repeated for that edit.
Later documentation edits require documentation checks, not another kernel run.

## Screenshots and limits

[Editor screenshot](screenshots/desktop-medit.png) shows `medit /demo/edit.txt`
inside Portal, with the text `edited`, normal mode, cursor bar and dirty marker.
[Saved file screenshot](screenshots/desktop-medit-saved.png) shows the save message
and `cat` reading `edited` from disk. These were captured from the tested build;
automated disk-content assertions establish save and discard behavior.

The debug heap stress suite remains failing: `cargo test --test heap_allocation`
times out at `when_many_boxes_are_allocated_then_memory_is_allocated` after the
first four cases pass. The same command on unchanged `3277448` in an isolated
`/tmp/rustos-files-baseline` worktree reproduces that timeout. Its assertion and
60-second timeout were not changed. The release variant passes. Bootloader builds
also retain the existing unused `rustc-abi` target-field warning.

Manual host GUI input/capture remains unverified; QMP exercises guest keyboard and
pointer behavior. No host-specific GUI compatibility claim is made. The editor
supports ASCII/LF files up to 16 KiB; it has no undo or arrow-key handling. Creating
a new edit target creates an empty file immediately. Regular-file descriptor
writes initially staged complete files up to 1 MiB; the subsequent capability
expansion removes that file limit and uses offset writes. Each read/write transfer
is still at most 64 KiB. Filesystem replacement protects against tested disk-full failures,
not power loss or hardware errors preventing rollback. No Linux binary ABI,
permissions model, general PTY, pipelines, rename/unlink or descriptor inheritance
is claimed.
