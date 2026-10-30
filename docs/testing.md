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
| Networking/PCI/NIC/client transports | Kernel + memory + syscall/SDK + scheduler cleanup rows; [network suites](networking.md#running-and-testing), packet assertions, missing NIC, timeouts, ring wrap and real Ring 3 UDP/TCP/HTTP probe |
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
cargo test --test display_mapping
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
python3 -B e2e/http.py
python3 -B e2e/tls.py --site https://google.com/
python3 -B e2e/tls.py --no-rng
python3 -B e2e/tls.py --bad-clock
python3 -B e2e/net_perf.py --label current --assert-fast
python3 e2e/desktop_perf.py
bash e2e/build_profile.sh
rustc --edition=2021 --test user_programs/wm/src/damage.rs -o /tmp/rustos-damage-tests
/tmp/rustos-damage-tests
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
The desktop test also asserts a 1920×1080 framebuffer, full-width panels, edge
pointer coordinates and dragging to the far display edge. `display_mapping` checks
large compositor mappings, padded pitch, final partial pages, independent snapshots,
preserved parent code and three pairs of spawn/kill/reap operations. Existing frame/stack leaks
remain; a bounded repetition test does not establish resource reclamation.
See [1080p validation](1080p-validation.md) for recorded red/green evidence and limits.
The desktop and CPU tests now boot private copies of the development disk,
so file operations in the test cannot mutate an interactive VM's image. It needs Pillow and permission to
create local QEMU monitor/QMP sockets.

For a manual input check, run `cargo run` on the reported host, move the pointer
across the display, launch an icon, drag and close a window, type into the focused
terminal, and verify host/guest coordinate alignment. Record host backend/session
and observations. If unavailable, explicitly leave that criterion unverified.

## Networking checks

See [networking](networking.md#running-and-testing) for host and QEMU commands.
Run the pure net ABI, stack and ping crates from outside the root bare-metal Cargo
configuration, explicitly targeting the host. Exercise allocation-failure recovery,
PCI decoding and page-table permissions as well. Runtime tests use independent
disk copies and local UDP/HTTP servers. Live 8.8.8.8 acceptance is separate from
local deterministic tests and depends on the host's ICMP support. The network
probe also exercises spawn with live socket buffers, blocked waitpid return/reap,
foreign handles, quotas and close/exit/kill cleanup. Record each result and actual
captured packets; do not infer transport behavior from unit harness compilation.

TLS checks and host commands: [TLS](tls.md), with recorded [validation](tls-validation.md).
The HTTPS harness uses disposable OpenSSL certificates, an independent host TLS server,
private disk copies, and real Ring 3 callers. OpenSSL is a host test prerequisite.

## Known baseline gaps

Reproduced on pulled `main` commit `26edbc9` when preparing these guides:

- The unit harness's assumed framebuffer address previously caused an abnormal exit
  before any cases. It now uses mapped RAM. This exposed a read-validation test
  panic because focus was checked before invalid arguments; validation now runs first.
- The standalone keyboard/timer harness previously failed with `ticks: 0 -> 0`.
  It now unmasks IRQ0 explicitly after initialization, retaining its tick assertion.
- The debug heap suite's 33,554,432 allocation stress loop timed out at 60 seconds
  during performance work. The unchanged release suite passes. Do not raise the
  timeout, reduce the loop, or report the debug suite as passing.
- `tests/userspace.rs` currently tests constants/vector selection. Despite its older
  comments, it does not execute a user program and does not prove Ring 3 transitions.
- `e2e/grub_test.py` expects old mode-13h/loading messages; it is not the current
  desktop acceptance test. `e2e/forge.sh` references a missing `forge_test.py` helper.
  Other old scripts may also assume superseded boot paths; inspect before relying on them.
- Bootloader dependency builds can emit an unsupported `rustc-abi` target-field
  warning under the pinned nightly. Distinguish dependency warnings from project warnings.

After expanding the kernel heap to 64 MiB, the same debug stress loop runs
67,108,864 allocations because it iterates HEAP_SIZE. Its assertion and 60-second
timeout remain unchanged. [Expanded-limit validation](http-limits-validation.md)
records current results and the boot-frame allocation performance fix.

The remaining gaps are repair targets, not permission to skip required coverage or to report green.
For unrelated documentation changes, disclose this baseline without pretending to
have repaired kernel behavior. Runtime changes touching these paths must add the
missing regression coverage and resolve or explicitly report outstanding blockers.

## Evidence to include before saying done

Every PR needs concrete evidence that it works. Include a screenshot whenever
practical; for visible behavior, capture it from the build under review and identify
the acceptance criterion shown. Link/attach it in the PR alongside test evidence.
If a screenshot is inappropriate or unavailable, explain why and include suitable
actual output, logs, traces, or a reproducible example. A screenshot never replaces
behavioral assertions, and passing output must not be invented.

Report the failing regression and why it failed, then the exact green commands and
behaviors tested. List required checks that failed/could not run, unexpected warnings,
manual checks still needed, and remaining risks. Do not claim exhaustive security,
correctness, cleanup, or host compatibility from limited QEMU tests. Inspect final
artifacts and the complete diff after tests; do not silently weaken acceptance criteria.
