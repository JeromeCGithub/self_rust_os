# 1080p performance work

The requested behavior is responsive input and CPU use proportional to useful work.
Desktop policy remains in Ring 3. The kernel adds timed blocking, surface publication
metadata, and nonempty prefix snapshots; PID 0 retains exclusive display/snapshot
privileges. Existing user-pointer validation limitations still apply.

## Optimizations

1. **Retain the composed screen and present dirty tiles.** The wallpaper is restored
   only for changed window layouts. Unchanged applications are not recomposed. Mouse
   movement restores old cursor pixels from the retained scene and updates up to eight
   32×32 tiles (~32 KiB), rather than copying 8.29 MB on every iteration. Adjacent tiles
   are copied as runs; actual dimensions and pitch clip the final tiles.
2. **Cache window content using revisions.** Graphical apps publish after drawing;
   console writes publish automatically. The compositor queries small revision/exit
   values and snapshots only changed content. Cached content remains available when
   stacking, dragging, exposing, or closing windows. Six reusable cache slots cost
   3,373,056 bytes (3.22 MiB), plus 272 bytes of dirty tile metadata. No third full-screen
   cache is added. The existing full-size surfaces/scratch storage remain.
3. **Snapshot fewer bytes.** Ordinary windows use only the first 488 pixels of 288
   rows. Their contiguous snapshot prefix is 2,206,112 bytes at 1080p instead of
   8,294,400 bytes (~73% less per copy). Scaled Snake still needs the full source.
4. **Sleep while waiting.** The compositor, Portal and Snake use scheduler timed
   waits rather than repeatedly entering the kernel to read ticks. With no runnable
   process, the CPU halts. Timer wakeups also work from the kernel idle path, reserve
   their queue capacity outside IRQs, and cannot resurrect killed sleepers.
5. **Render Portal only on changes.** Input batches and cursor blink changes trigger
   drawing/publication; an unchanged terminal retains its pixels. Input is processed
   before drawing, avoiding another frame of delay.
6. **Drain keyboard control events.** Nonblocking reads skip queued key releases and
   modifiers until a character or an empty queue, bounded by the queue capacity.
   Portal consumes a bounded batch of characters per iteration. Releases no longer
   impose a tick of delay for each queued character. Modifier decoding stays shared
   with the blocking read path.
7. **Ship the optimized kernel.** The disk builder now installs the release ELF.
   `cargo build` still supplies a debug build for development. No toolchain, target,
   dependency, CPU feature, cache mapping, or emulator acceleration change is required.

## Executed red/green evidence

Measurements use QEMU software emulation, a 1920×1080 display, 256 MiB guest RAM and
Linux `/proc` accounting. CPU percentages are QEMU process CPU time divided by elapsed
wall time, expressed as a fraction of **one host core**, not guest hardware counters.
These short, bounded samples are regression checks, not universal hardware benchmarks.

| Check | Observed red | Observed green |
| --- | --- | --- |
| Empty desktop CPU, 3-second sample | 99.7% | 0–0.3% |
| Idle Portal CPU, 2-second sample | 100%; 26–31% after intermediate changes | 3–3.5% |
| Snake launch/wait with Portal, 1-second sample | 99% | 11% |
| Pointer, eight moves including screen edges | maximum 119 ms | maximum 62–63 ms |
| Pointer with Spin making progress | unmeasured baseline | 41–97 ms across two runs, with focused terminal command succeeding |
| Keyboard burst drain after last injected key | measured 333 ms; failed 150 ms budget | 0 ms at polling resolution |
| Partial snapshot | terminal text pixels missing with old dispatcher | actual text pixel assertion passes |
| Dirty tile unit cases | full repaint damaged unrelated rows/tiles | clipping, consumption, overflow and pitched-buffer guards pass |
| Timed wait | immediate wait returned error in red stub | zero/error cases, yield, timer wake, saved RAX and kill/reap checks pass |
| Build profile | installed ELF differed from release artifact | extracted disk ELF matches release artifact |

Commands are in [the testing guide](testing.md). `desktop_perf.py` checks empty and
application CPU budgets, pointer pixels, typing bursts, and progress/focused input
while Spin intentionally saturates a CPU. The full `desktop.py` separately verifies
shell commands, file operations, simultaneous apps, stacking, dragging, closing and
edge coordinates. Screenshots are from that rebuilt/tested disk, including
[the terminal](screenshots/desktop-terminal.png) and
[the far-edge drag](screenshots/desktop-1080p-drag.png).

The unit harness now uses RAM framebuffer backing so unit cases actually run.
The existing invalid-read tests then exposed focus checks preceding validation;
arguments are now checked first. The keyboard/timer harness explicitly unmasks IRQ0.
Those harness/validation repairs retain the original failing assertions.

Final checks executed for this change:

- `cargo build`, `cargo build --features grub`, `cargo build --release --features grub`
  and `cargo test --no-run` succeeded.
- `cargo test --lib --test basic_boot --test gfx --test display_mapping --test keyboard_timer --test userspace --test should_panic --test stack_overflow`
  passed, including the timer/publication checks within `display_mapping`.
- `cargo test --release --test heap_allocation` passed four cases; the debug command
  failed its existing timeout as disclosed below.
- Four native damage/presentation tests, `bash e2e/build_profile.sh`,
  `python3 e2e/desktop_perf.py` and the complete `python3 e2e/desktop.py` passed.
- All apps were regenerated through `tools/build-user.sh` via fresh disk builds.
  The documentation checker, its seven host tests, shell syntax checks and
  `git diff --check` passed. The bootloader's pre-existing unused `rustc-abi`
  target-field warning remains.

## Limits

- The PIT remains ~18.2 Hz, with ~55 ms scheduling/input polling intervals. This work
  removes wasted processing and queue backlog; it does not establish 60 Hz animation.
- The tests inject QMP input. Manual host GUI input/capture on the user's display
  backend, physical hardware performance and actual energy use remain unverified.
- A debug heap stress test still times out at 60 seconds; all four unchanged release
  heap cases pass. That debug failure is not hidden by the release result.
- Publication is notification, not vsync or atomic double buffering. Graphical clients
  must publish after drawing. Existing process frame/stack leaks remain unchanged;
  bounded repeated launch/close tests do not establish complete reclamation.
- Snake's existing initial head lies outside its logical grid and reaches game over
  early. The CPU sample covers its launch/wait transition, not sustained gameplay.
  Correcting its game logic is outside this performance change.
