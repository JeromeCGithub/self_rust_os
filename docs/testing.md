# Testing and completion contract

Tests must drive implementation. Follow [the mandatory TDD rules](../AGENTS.md) and
[red/green workflow](development.md). Every changed behavior needs executable evidence
at the layer where it can fail. “Tests everything” means all affected behaviors and
integration paths, including error paths, not an unsupported claim of exhaustive proof.

## Test layers

- Kernel unit cases use `#[test_case]` inside `src/`; `cargo test --lib` boots their
  custom harness in QEMU. Pure FAT32 logic also has in-memory mock coverage.
- Kernel integration executables in `tests/` boot with the legacy bootloader. Use
  `tests/common/` RAM framebuffer backing. They do not boot the GRUB desktop.
- The GRUB desktop E2E boots a rebuilt disk and drives real syscalls, processes,
  mouse, keyboard and app behavior. It injects absolute QMP pointer events.
- Host Python tests validate tooling. Documentation checking validates presence and
  local inline links/anchors, not remote URL availability or technical correctness.
- Manual GUI checks establish actual host display/input behavior. QMP and a screenshot
  cannot prove pointer integration works on COSMIC/Wayland/X11.

## Change-to-test matrix

Run the union of rows matching the change. A blocked or failing required test remains
an explicit verification gap; do not replace it with `--no-run` and claim completion.

| Changed area | Required verification |
| --- | --- |
| Documentation only | Documentation checker, its complete host test suite, review technical claims against source, `git diff --check` |
| Host build/test tooling | Failing host regression first; shell syntax/Python tests as applicable; execute changed build/test flow; rebuild and boot if image/runner behavior changes |
| Kernel Rust | Both default and GRUB builds, GRUB release build, `cargo test --no-run`, complete applicable unit and integration suites |
| Frames/paging/heap | Kernel row + allocation/boundary/failure cases, heap tests, process isolation and repeated launch/exit tests in QEMU |
| Interrupts/scheduler/input | Kernel row + keyboard/timer, process progress under CPU load, blocking/wakeup, focused input, kill/exit/wait, desktop E2E |
| Syscalls/userlib/runtime | Kernel row + invalid argument/error tests, rebuild **all** apps, desktop E2E proving actual Ring 3 behavior and SDK compatibility |
| ATA/MBR/FAT32 | Kernel row + parser/mock cases, ATA and FAT32 fixture integration tests, file operations through userspace when ABI/mount behavior changes |
| User apps/desktop | Red interaction test, rebuild all apps/disk, complete desktop E2E; inspect screenshots and applicable shell/error cases |
| Boot/linker/targets | Both boot-path builds, release build, bootimage smoke test, fresh GRUB boot/E2E; verify serial bring-up and usable framebuffer |
| Graphics/pointer/GUI | Desktop row + pixel/geometry/pitch boundaries and interaction assertions; manual host GUI test for a host-specific claim |

## Build and kernel commands

```bash
cargo build
cargo build --features grub
cargo build --release --features grub
./tools/build-user.sh
cargo test --no-run
cargo test --lib
cargo test --test basic_boot --test gfx --test heap_allocation --test userspace
cargo test --test should_panic --test stack_overflow
cargo test --test keyboard_timer
./e2e/ata.sh
./e2e/fat32.sh
```

These commands are a matrix reference, **not a declaration that every suite currently
passes**. Run relevant tests sequentially when writable disks overlap. Standard QEMU
tests use `isa-debug-exit` on port `0xf4`: host status 33 denotes success and 35 denotes
failure. Bootimage translates its configured success status into a successful test run.
A crash, timeout, expected-looking log, or empty filtered harness is not a pass.

`cargo test` attempts all test executables, including the nonterminating `gfx_demo`;
it is not currently a reliable unattended green gate. Identify selected suites and
case counts explicitly. Do not disable failures or demos in order to present a clean run.

Fixtures attached by [Cargo metadata](../Cargo.toml) are:

| Fixture | ATA placement | Write ownership |
| --- | --- | --- |
| `tests/fixtures/disk-ata.img` | Primary slave | ATA integration cases |
| `tests/fixtures/disk-fat32.img` | Secondary master | FAT32 integration cases (format/write/delete) |

Both scripts provision disposable 16 MiB fixtures if missing. Do not substitute a
host device. `file.locking=off` permits attachment; it does not make concurrent writes safe.

## Desktop and documentation commands

```bash
./tools/build-grub-disk.sh
python3 e2e/desktop.py
python3 -B -m unittest discover -s tools/tests -p 'test_*.py'
python3 -B tools/check_docs.py
git diff --check
```

The desktop test builds no image itself: rebuilding first is mandatory. Its current
checks cover wallpaper/launch readiness, Portal commands and file writes/reads,
concurrent Snake, focus restoration, drag/close interactions, and launching Forge,
Hello and Spin. Review the actual assertions before making stronger coverage claims.
For example, an echoed command does not by itself verify successful execution.
Screenshots go to `docs/screenshots/desktop*.png`; inspect changes before committing.
The test writes files inside the development disk. It needs Pillow and permission to
create local QEMU monitor/QMP sockets.

For a manual input check, run `cargo run` on the reported host, move the pointer
across the display, launch an icon, drag and close a window, type into the focused
terminal, and verify host/guest coordinate alignment. Record host backend/session
and observations. If unavailable, explicitly leave that criterion unverified.

## Known baseline gaps

Reproduced on pulled `main` commit `26edbc9` when preparing these guides:

- `cargo test --lib` exits abnormally before test-case output. Its startup passes a
  fixed `0xffff8000000a0000` framebuffer address even though console initialization
  clears it. Inspect mappings and startup before assuming any unit case executed;
  the observed abnormal exit alone does not establish the precise fault cause.
- `cargo test --test keyboard_timer` fails with `ticks: 0 -> 0`. Kernel initialization
  masks IRQ0 until scheduler startup; this standalone test never starts the scheduler
  or explicitly unmasks the timer. Preserve the failing assertion when repairing it.
- `tests/userspace.rs` currently tests constants/vector selection. Despite its older
  comments, it does not execute a user program and does not prove Ring 3 transitions.
- `e2e/grub_test.py` expects old mode-13h/loading messages; it is not the current
  desktop acceptance test. `e2e/forge.sh` references a missing `forge_test.py` helper.
  Other old scripts may also assume superseded boot paths; inspect before relying on them.
- Bootloader dependency builds can emit an unsupported `rustc-abi` target-field
  warning under the pinned nightly. Distinguish dependency warnings from project warnings.

These are repair targets, not permission to skip required coverage or to report green.
For unrelated documentation changes, disclose this baseline without pretending to
have repaired kernel behavior. Runtime changes touching these paths must add the
missing regression coverage and resolve or explicitly report outstanding blockers.

## Evidence to include before saying done

Report the failing regression and why it failed, then the exact green commands and
behaviors tested. List required checks that failed/could not run, unexpected warnings,
manual checks still needed, and remaining risks. Do not claim exhaustive security,
correctness, cleanup, or host compatibility from limited QEMU tests. Inspect final
artifacts and the complete diff after tests; do not silently weaken acceptance criteria.
