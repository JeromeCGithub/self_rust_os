# DE separation and glass validation

This change follows the component toolkit at `79a3a89`. Results below were run
for the two-process desktop and Cosmic Cliffs build on October 29, 2026; older
[UI validation](ui-validation.md) describes earlier builds.

## Behavior and acceptance criteria

- PID 0 WM performs composition, decorations, placement, focus/input routing and
  cached surface management. A separate `/BIN/DE.BIN` owns wallpaper, panels,
  discovery and launchers, and asks the compositor for a background/workspace and
  managed windows. Only the compositor-spawned DE may use shell requests.
- The installed default wallpaper is the credited NASA/ESA/CSA/STScI image in
  [asset provenance](../user_programs/de/WALLPAPER.md). The actual framebuffer must
  match selected image pixels, with panels/launchers rendered by the DE.
- Glass preserves background color with readability-aware dimming, smooth blur
  reconstruction, directional edge lighting, rounded corners and softer shadows.
  Button hover and captured press have distinct visual feedback; text stays sharp.
- DE frames use copy/release ownership. Boot publishes wallpaper then controls;
  the compositor must cache both frames, preserving pitch and memory boundaries.
- Killing the DE must remove its desktop surface/reservations while another client
  can still read input, render output and close through the compositor.

## Red evidence

- Native shell protocol tests failed for missing role authorization, workspace
  decoding and legacy registration. The original desktop E2E failed waiting for
  `/BIN/DE.BIN` because the old WM was still the desktop shell.
- New material tests failed because smooth sampling, readability tinting and
  directional lighting were absent. The safe pitched-surface copy regression
  initially returned false instead of copying valid rows.
- Moving panel policy out of SDK geometry was driven by a failing case: content
  that fits the display plus titlebar was rejected using the old fixed panel
  reservation. The compositor now enforces the DE's negotiated workspace.
- The ordinary-app role probe initially failed its runtime readiness assertion;
  Hello now asserts a real REJECT reply before creating its own normal window.
- The shell-exit test exposed repeated input in Forge (`kkkiiillllll   111`). A
  focused `cargo test --test display_mapping` regression failed with saved RAX 2
  rather than 0 after a keyboard wake. `schedule` now initializes the keyboard
  waiter's return value to zero, so the caller retries the untouched user buffer.
  The test also proves a killed keyboard waiter cannot be resurrected.
- After that fix, the shell-exit test reached the intended cleanup failure:
  background pixel `(1800,900)` remained `(49,31,31)` instead of neutral
  `(14,23,43)`. Disconnect now clears the desktop cache and workspace reservations.
  A subsequent background assertion initially sampled the software cursor; the
  test now moves it away before inspecting the exposed scene. The cleanup deadline
  remains five seconds; no existing timeout/CPU threshold was increased.

## Checks run and observed green

```bash
CARGO_NET_OFFLINE=true ./tools/build-grub-disk.sh
CARGO_NET_OFFLINE=true bash e2e/build_profile.sh
cargo build
cargo build --features grub
cargo test --no-run
cargo test --lib --test basic_boot --test gfx --test display_mapping --test keyboard_timer --test userspace --test should_panic --test stack_overflow
cargo test --release --test heap_allocation
rustc --edition=2021 --test user_programs/userlib/src/ui/host_tests.rs -o /tmp/rustos-de-host-tests
/tmp/rustos-de-host-tests
rustc --edition=2021 --test user_programs/wm/src/glass.rs -o /tmp/rustos-de-glass-tests
/tmp/rustos-de-glass-tests
rustc --edition=2021 --test user_programs/wm/src/damage.rs -o /tmp/rustos-de-damage-tests
/tmp/rustos-de-damage-tests
python3 e2e/desktop.py
python3 e2e/glass_ui.py
python3 e2e/desktop_perf.py
python3 -B tools/check_docs.py
python3 -B -m unittest discover -s tools/tests -p 'test_*.py'
git diff --check
```

All completed successfully. The disk builders also run the release GRUB build and
`tools/build-user.sh` for every application, regenerating binary/metadata pairs.
The build-profile check extracts its disposable image's kernel and compares it
with the release ELF. Compilation and `--no-run` are listed separately from the
actual kernel/QEMU execution. Release heap coverage passed four cases; native
coverage passed eighteen SDK, six glass and four damage cases, plus seven docs
checker cases. No toolchain, linker layout, CPU features or dependency versions
were changed. New syscall numbers were not required.

The final desktop → glass → performance sequence passed. Desktop assertions now
cover separate process startup, two desktop commits, exact wallpaper pixels,
commands/file writes, legacy/native clients, drag/close/focus, and continued
foreground command execution/window controls after the real DE PID is killed.
Glass assertions include ordinary-client role rejection, sharp foreground text,
component state/style changes, captured press feedback, changing lower layers,
drag restoration and close/reopen.

Final QEMU software-emulation performance sample: empty desktop 0.3%, idle Portal
7.5%, Portal/Snake 19% of one host core; unloaded pointer 59 ms, typing burst drain
20 ms, pointer under Spin 41 ms. These short samples passed unchanged budgets,
not a universal hardware latency or 60 Hz guarantee. An early press-feedback
observation failed while kernel QEMU suites ran concurrently; isolated GUI checks
and the final sequential acceptance suites passed without relaxing assertions.

## Screenshots from the tested build

- [Desktop](screenshots/desktop.png): default image, shell-owned panels/launchers,
  and visible image credit.
- [Clear glass app](screenshots/desktop-glass.png): colored backdrop through the
  new material, sharp text and rounded controls.
- [Hover](screenshots/desktop-glass-hover.png) and
  [captured press](screenshots/desktop-glass-pressed.png): control feedback.
- [Overlapping clients](screenshots/desktop-glass-overlap.png): lower client
  content contributes to the glass while the foreground stays readable.
- [Compositor without its DE](screenshots/desktop-compositor-alone.png): the shell
  is gone but the foreground Forge window still renders on the neutral background.

## Remaining verification and limits

Manual host GUI input/capture was not performed. QMP input does not establish
Wayland/X11 integration. The known debug heap stress timeout documented in
[testing](testing.md#known-baseline-gaps) was not rerun for this change; its earlier
failure remains visible rather than being replaced by the passing release result.
The bootloader's existing `rustc-abi` warning remains.

There is no automatic DE restart/session manager. Existing physical-frame and
kernel-stack leaks, incomplete user-pointer hardening and process-control
permission limitations remain. Visual teardown/reaping does not prove complete
resource reclamation. The toolkit still exposes one surface per process, six app
windows, bounded current viewport sizes, CPU effects and approximately 18.2 Hz
polling. Optical refraction, GPU acceleration and fluid 60 Hz animation are not
claimed by these changes.

## Architecture and application entry refactor (2026-10-30)

The WM and DE were reorganized by responsibility; see the
[source module map](desktop-environment.md#source-organization). Renderer buffers
are private to the display module, and application launch/rollback policy belongs
to the DE application module. This preserves the existing protocol and privilege
boundaries. A shared SDK executable root now supplies target attributes and bridges
ordinary application `fn main()` to the existing runtime in all seven binaries.
See the [application convention](development.md#writing-an-application).

Red: after converting Hello to ordinary `fn main()` without the shared root,
`CARGO_NET_OFFLINE=true ./tools/build-user.sh hello` failed with E0463:
“can't find crate for std”. This demonstrated the missing target integration.
The module refactor itself preserved behavior and started from passing desktop,
glass (6 cases), and damage (4 cases) baseline checks.

Checks run on this change:

- `CARGO_NET_OFFLINE=true ./tools/build-user.sh` and
  `CARGO_NET_OFFLINE=true ./tools/build-grub-disk.sh`: all seven applications,
  matching flat images/metadata, and the release GRUB disk rebuilt.
- `python3 e2e/desktop.py`, `python3 e2e/glass_ui.py`, and
  `python3 e2e/desktop_perf.py`: passed on the fresh shared-root build.
  The desktop test launches all five ordinary apps and checks compositor/DE
  lifetime separation. Glass checks preserve button capture, lower-layer updates,
  focus, close and reopen. Performance retained existing thresholds (idle 0%,
  terminal 8.5%, terminal/Snake 19%, loaded pointer 41 ms on this run).
- `cargo build`, `cargo build --features grub`, `cargo test --no-run`:
  passed; release GRUB compilation also passed through the disk builder.
- `cargo test --test display_mapping --test userspace --test basic_boot`
  and `cargo test --lib --test gfx --test keyboard_timer --test should_panic --test stack_overflow`:
  passed.
- Native Rust host harnesses: UI 18, glass 6, damage 4 cases passed.
- Documentation checker, all 7 checker regression cases and whitespace checks passed.

The screenshots produced by the desktop/glass tests show unchanged presentation;
`desktop-glass.png` was inspected from this run. Screenshots cannot establish
module boundaries; those are visible in source dependencies and private buffer
visibility. No manual host GUI check was performed. The existing debug heap timeout
was not rerun or fixed; memory reclamation and other previously documented OS
limitations remain. The bootloader still emits its existing unused `rustc-abi`
target-field warning. Full Rust `std` support is not provided by the new wrapper.
