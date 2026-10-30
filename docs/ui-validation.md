# Native UI validation

Implementation and checks were performed on branch
`perf/1080p-desktop-responsiveness`, based on `c906002`, during October 28–29, 2026.
The results below distinguish this change from historical performance evidence.

## Behavior and boundaries

The public Ring 3 SDK now provides an owned window, safe clipped canvas, text,
vertical layout, headings, labels, cards, buttons and material requests. Portal
uses the canvas API; Hello demonstrates widgets in a 640×400 clear glass window.
The WM owns decorations, placement, focus, stacking, backdrop blur and composition.
Kernel additions are bounded scalar inboxes and authenticated sender identities,
without user pointers or rendering policy. Existing syscall numbers are preserved.

## Observed red tests

- `cargo test --test display_mapping` failed at “clients can contact the display
  owner”: the initial transport returned `18446744073709551614` instead of zero.
- Native raster tests failed for missing clipping, invalid geometry acceptance,
  and translucent drawing (`0xff0000ff` instead of `0xff80007f`).
- Protocol tests failed to decode valid geometry. Blur tests failed because a
  constant backdrop was not copied and an impulse did not spread.
- Input tests failed to activate on release; a further regression reproduced a
  false click when a press outside and release inside arrived in one frame.
- The large-rounded-geometry test panicked with “attempt to multiply with
  overflow”; wider integer intermediates fixed it.
- The workspace-boundary test initially accepted 640×400 content on a 640×480
  display, leaving insufficient room for panels/titlebar. SDK and WM now reject it.
- `python3 e2e/glass_ui.py` on the baseline disk failed waiting for the native
  glass app. After implementation it checks framebuffer pixels and interactions,
  not just the readiness log.
- The first renderer exceeded existing CPU budgets: 35.5% for idle Portal and,
  after an initial optimization, 37% for Portal with Snake. Fast coverage paths,
  low-resolution tinting, and skipping backdrop processing behind opaque legacy
  content reduced the cost. Thresholds and timeouts were not increased.
- Loaded-pointer and focused-input deadlines also failed intermittently, including
  an isolated final run. The WM now refreshes only the cursor at bounded row
  intervals during composition, saving/restoring pixels from the displayed scene
  rather than publishing an unfinished composition.

## Verification run for this change

The following commands completed successfully:

```bash
cargo build
cargo build --features grub
cargo build --release --features grub
cargo test --no-run
cargo test --lib --test basic_boot --test gfx --test display_mapping --test keyboard_timer --test userspace --test should_panic --test stack_overflow
cargo test --release --test heap_allocation
CARGO_NET_OFFLINE=true ./tools/build-grub-disk.sh
bash e2e/build_profile.sh
rustc --edition=2021 --test user_programs/userlib/src/ui/host_tests.rs -o /tmp/rustos-ui-tests
/tmp/rustos-ui-tests
rustc --edition=2021 --test user_programs/wm/src/glass.rs -o /tmp/rustos-glass-tests
/tmp/rustos-glass-tests
rustc --edition=2021 --test user_programs/wm/src/damage.rs -o /tmp/rustos-damage-tests
/tmp/rustos-damage-tests
python3 -B tools/check_docs.py
python3 -B -m unittest discover -s tools/tests -p 'test_*.py'
git diff --check
```

All applications and tracked binary/metadata pairs were rebuilt. Offline Cargo
mode uses the already cached font dependency; the pinned toolchain, dependency
versions and target/linker configuration were preserved. There are nine native
SDK cases, three blur cases, four existing damage cases, and seven documentation
checker cases. Kernel suites include actual QEMU execution; `--no-run` is listed
only as an additional compilation check. Release heap tests passed four cases.

The real GRUB desktop also passed the glass, full desktop, and performance suites
during validation. The glass suite asserts negotiated size, transparent rounded
corners, sharp foreground text, press/release capture, cancellation outside,
drag restoration, close/reopen, focus isolation, and live changes below stationary
glass. Full desktop coverage retains shell/file commands, legacy app coexistence,
focus restoration, dragging and close. The fixed-color titlebar assertions were
replaced with close-control pixels because glass intentionally samples its backdrop;
the new Hello content width is reflected in its close coordinates.

An earlier isolated performance run measured 0.3% empty-desktop CPU, 7.5% idle Portal,
20% Portal/Snake CPU, 62 ms unloaded pointer response and 21 ms keyboard burst
drain. Its loaded-pointer observation was about 202 ms including screenshot
polling. These are short QEMU software-emulation measurements, not hardware or
60 Hz guarantees. Passing that run did not establish stable loaded latency:
later failures drove the cursor update change described above.

After the cursor change, the complete unchanged performance suite passed twice.
The final sequential glass → desktop → performance run passed all three suites.
That final performance run measured 0.0% empty-desktop CPU, 7.5% idle Portal,
20% Portal/Snake CPU, 62 ms unloaded pointer response, 21 ms keyboard burst drain,
and 7 ms pointer response under Spin. The preceding post-fix run measured 20 ms
for the loaded pointer. Focused terminal input under Spin passed in both runs.
These observations do not establish a universal latency bound.

Screenshots captured from rebuilt, tested disks:

- [Glass application](screenshots/desktop-glass.png): SDK heading, button and card.
- [Overlapping native apps](screenshots/desktop-glass-overlap.png): the lower
  terminal is visible through the upper material while foreground text stays sharp.
- [Dragged glass window](screenshots/desktop-glass-drag.png): new backdrop and
  restored old window location.
- [Terminal](screenshots/desktop-terminal.png): migrated shell after real commands.

## Remaining limits and failures

`cargo test --test heap_allocation` still times out at the existing 60-second
stress-case limit, as documented in [the baseline gaps](testing.md#known-baseline-gaps).
The first three debug cases passed; the stress assertion and timeout were preserved.
The existing bootloader `rustc-abi` target-field warning remains.

Manual host GUI input/capture was not performed. QMP tests do not establish host
Wayland/X11 integration. Hardware performance, dynamic resizing, multiple windows
per process, keyboard widget navigation, system accessibility preferences, GPU
refraction and fluid animation remain outside this first toolkit implementation.
Existing process frame/stack leaks and pointer-validation hardening gaps remain;
bounded close/reopen tests do not prove complete resource reclamation.


## Declarative component follow-up (October 29, 2026)

`Window::mount(state, app, update)` and `rsx!` now let application/component
functions return typed views and style functions. Hello was migrated to `app`,
`appearance_card`, `app_style`, and `update`. Rendering/layout/style interpretation
remains in Ring 3; this follow-up changes no syscall or compositor protocol.

Red evidence recorded during this follow-up:

- The two new layout tests emitted zero primitives instead of the expected nested
  styled text/button slots. Both passed after the component renderer was implemented.
- A further boundary case emitted text when padding exhausted the available width;
  rendering now skips empty content widths and viewports.
- The strengthened `python3 e2e/glass_ui.py` failed on the original disk with
  `component foreground style missing`. The new build passes that assertion and
  asserts that a click changes the foreground color before another pointer event.

Checks run now: thirteen native SDK tests (including component composition,
spacing/inherited styles, stable keys with changing labels, empty viewports and
pre-render nesting rejection), three blur and four damage cases; default/GRUB
kernel builds, release GRUB build, `cargo test --no-run`, and the selected complete
kernel/QEMU command listed above. All dependent apps and tracked binary/metadata
pairs were rebuilt using `tools/build-user.sh` through a fresh disk build.
The full glass and desktop E2E passed, as did the unchanged desktop performance
suite (idle Portal 8.5%, Portal/Snake 21%, unloaded pointer 61 ms, pointer under
Spin 18 ms, keyboard burst drain 31 ms). The docs checker, its seven host tests,
and `git diff --check` passed. These are current results, separate from the
original implementation results above. Screenshots were regenerated and inspected
from this component build.

The debug/release heap suites were not rerun for this pure userspace component
change; their earlier timeout/pass evidence remains historical. Manual host GUI
input remains unverified. This is native RSX-inspired syntax, not Dioxus
compatibility: the first layer has typed columns/text/cards/buttons, explicit
messages, and static or borrowed text, with a fixed root view type. It does not
supply hooks, CSS, dynamic-length lists or owned formatted text. Existing canvas
and imperative UI APIs remain supported.
