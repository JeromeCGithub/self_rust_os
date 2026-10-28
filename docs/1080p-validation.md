# 1080p validation

Tested on 2026-10-28, based on `main` at `3fc9560`.

GRUB and the Multiboot2 header request 1920×1080×32. The userspace compositor
uses reported dimensions and pitch for wallpaper, panels, snapshots, presentation
and drag bounds. The kernel maps the complete display or a client's private
surface, rounding up the final page. The stack/display address migration and
required application rebuild are documented in [the ABI](syscalls.md).

The kernel still owns mapping and isolation; desktop layout stays in Ring 3.
All apps and their binary/metadata artifacts were regenerated with
`tools/build-user.sh` through the disk builder.

## Red evidence

These regressions were executed before their corresponding fixes:

| Command | Observed failure |
| --- | --- |
| `python3 e2e/desktop.py` after a fresh baseline disk build | `expected 1920x1080 framebuffer, got (640, 480)` |
| `cargo test --test userspace` | `1080p compositor buffers overlap the user stack` |
| `cargo test --test display_mapping` | Large image loaded, then `failed to map user stack` |
| `cargo test --test heap_allocation` | Two 1080p buffers left insufficient heap for the additional 1 MiB allocation |
| `cargo test --test userspace` before the oversized-image guard | Returned `scheduler not initialized` instead of rejecting the image before resource access |

The new pointer assertions also exposed rounding in the QMP test's coordinate
encoding. Encoding now rounds up to match guest integer scaling. Drag tests wait
for the guest's drag-start diagnostic and the resulting framebuffer pixel before
releasing the button; timeouts were not increased.

## Checks run

| Command | Result |
| --- | --- |
| `cargo build` | Pass |
| `cargo build --features grub` | Pass |
| `cargo build --release --features grub` | Pass |
| `cargo test --no-run` | Pass; compilation only |
| `cargo test --test basic_boot --test gfx --test should_panic --test stack_overflow --test userspace --test display_mapping` | Pass |
| `cargo test --test display_mapping` after strengthening isolation and clipping assertions | Pass, 2 cases |
| `cargo test --release --test heap_allocation` | Pass, all 4 cases |
| `./tools/build-grub-disk.sh` then `python3 e2e/desktop.py` | Pass on the rebuilt 1080p image |
| Isolated image build and the same desktop script, as shown below | Pass on the final build |
| `python3 -B tools/check_docs.py` | Pass |
| `python3 -B -m unittest discover -s tools/tests -p 'test_*.py'` | Pass, 7 cases |
| `bash -n tools/build-grub-disk.sh` | Pass |
| `git diff --check` | Pass |

The mapping tests cover padded 7744-byte pitch, the last partial page, clipping
without changing row padding/trailing guards, zeroed client surfaces, two clients
writing distinct values through the same virtual address, parent code preservation,
snapshot independence and three pairs of spawn/kill/reap operations. Geometry tests
reject zero dimensions, arithmetic overflow, invalid pitch/alignment/format and
mapping capacity overflow. The E2E verifies 1080p dimensions, panel edges, pointer
edges, dragging beyond 640×480, shell file readback, focus and application launch.

The final desktop run used a private disposable image and an unchanged copy of
the test script, whose root-relative paths select that image:

```bash
mkdir -p /tmp/rustos-1080p-evidence/{e2e,target}
cp e2e/desktop.py /tmp/rustos-1080p-evidence/e2e/desktop.py
./tools/build-grub-disk.sh /tmp/rustos-1080p-evidence/target/grub-disk.img
python3 /tmp/rustos-1080p-evidence/e2e/desktop.py
```

Its five 1920×1080 screenshots were copied into `docs/screenshots` and inspected.

## Remaining failures and limits

- `cargo test --lib` exits abnormally before case output, reproduced before and
  after these changes. No unit cases are claimed to have passed.
- `cargo test --test keyboard_timer` fails with `ticks: 0 -> 0`; reproduced on
  untouched `3fc9560` and this change.
- Debug `cargo test --test heap_allocation` times out in the existing repeated
  allocation case. Untouched `3fc9560` also times out there. The full release
  suite passes with the larger heap and without changing assertions or timeouts.
- Bootloader test builds emit the existing unused `rustc-abi` target-field warning.
- A later desktop repeat failed file readback while the development image and
  generated GRUB configuration changed during execution. That run is a failure,
  not passing evidence. The final isolated disk repeat passed the complete suite.
- Manual host GTK/Wayland/X11 input/display checks remain unperformed. QMP
  injection establishes guest behavior only. Other physical video hardware and
  display modes were not tested.
- Existing process frame/stack leaks, incomplete user-pointer hardening and
  allocation-failure rollback remain outside this change. Bounded repetition
  does not establish resource reclamation or complete isolation hardening.

Screenshots show the tested [1080p desktop](screenshots/desktop.png),
[terminal](screenshots/desktop-terminal.png) and
[window dragged to the far corner](screenshots/desktop-1080p-drag.png).
