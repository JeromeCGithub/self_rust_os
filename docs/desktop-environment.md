# Compositor and desktop environment

The desktop consists of two Ring 3 programs:

| Program | Owns |
| --- | --- |
| `/BIN/WM.BIN`, PID 0 | Display presentation, window placement/stacking/decorations, focus and input routing, cached surfaces, glass effects |
| `/BIN/DE.BIN` | Wallpaper, panel geometry/content, launchers, application discovery, executable launch policy |

The kernel still boots the initial WM; the compositor then spawns the DE once.
This fixed path and PID 0 display authority are explicit bootstrap exceptions,
not a general session/capability manager. The kernel transports opaque scalar
messages and snapshots private surfaces; it interprets no desktop-shell protocol.

## Desktop client API

The DE calls `DesktopSurface::connect()` to request a full-display background
layer, then `reserve_work_area(28, 32)` for its panels. It draws through the same
safe clipped canvas used by native applications. `draw` publishes a serial; the
compositor copies the complete pitched surface into its own background cache
before replying RELEASE. Later compositions restore from that cache, so the DE
may safely redraw after acknowledgement. Background storage is separate from the
six application window slots.

Boot first publishes the wallpaper, then discovers apps and publishes the shell
controls. The two-frame sequence exercises cache/release ownership; the real
desktop test asserts both commits and the second frame's panels, along with
exact pixels from the default wallpaper.

Only the PID returned from the compositor's DE spawn may claim the desktop,
reserve workspace or register other clients. Ordinary application requests are
rejected using kernel-authenticated sender IDs. Hello's runtime regression probes
this rejection before creating its normal window. Desktop pointer events use
unsigned full-display coordinates; app window coordinates remain signed/local.
Occluded desktop content cannot receive launcher clicks. App pointer capture
continues to receive releases outside its content.

The DE discovers executables in `/BIN`, excluding WM and DE, and calls `spawn`
with an explicit path. `DesktopSurface::manage(pid, title, large)` then asks the
compositor to manage/focus the resulting surface. Native apps negotiate their own
window options as before. Legacy adapters use either a cropped 488×288 viewport
or a generic 640×480 source scaled into 488×288 content. Choosing that adapter for
Snake belongs to the DE; the compositor contains no application-name checks.
Rejected registrations cause the DE to kill/reap its newly spawned child.

If the DE exits or disconnects, the compositor removes its cached desktop and
workspace reservations, presents a neutral background, and keeps managing other
clients. The desktop E2E kills the actual DE PID from Forge, checks the exposed
background, executes another foreground command, and closes that window afterward.
There is no automatic DE restart/session manager yet. Kernel process frame/stack
reclamation remains incomplete; visual teardown is not proof of reclaimed frames.

## Shell message extension

The existing scalar transport syscalls and application protocol are preserved.
Message kinds A–E extend the [window protocol](ui.md#window-protocol-version-1):

| Kind | Direction | Payload |
| --- | --- | --- |
| A DESKTOP | DE → compositor | Zero payload; claim background role, receive ACCEPT/REJECT |
| B WORK_AREA | DE → compositor | Top pixels in bits 0–11, bottom 12–23; other bits zero; reply ACCEPT/REJECT with tag `0x08000000` |
| C MANAGE | DE → compositor | PID in bits 0–25, large legacy viewport flag 26; bit 27 zero; reply ACCEPT/REJECT with PID tag |
| D MANAGED_TITLE | DE → compositor | Three ASCII bytes and chunk index as TITLE, applied to the most recently accepted legacy registration |
| E DESKTOP_POINTER | Compositor → DE | Unsigned x 0–11, y 12–23, buttons 24–26 |

COMMIT/RELEASE also operate on the connected desktop surface. Workspace requests
must leave room for a decorated minimum window and all current windows; invalid
requests leave the previous workspace unchanged. Accepted changes clamp existing
windows into the new workspace. Control replies are retried ahead of coalesced
motion. The SDK retains the latest desktop pointer state while awaiting a
synchronous control reply, including a button release.

## Default image and glass

The default image is Webb's Cosmic Cliffs, credited to NASA, ESA, CSA, and STScI.
See [asset provenance and license](../user_programs/de/WALLPAPER.md).
No image download is required at build or boot. The DE loads `/ETC/WALL.PPM`;
the compositor caches rendered pixels without knowing the path or file format.

The glass treatment takes inspiration from Apple's
[materials guidance](https://developer.apple.com/design/human-interface-guidelines/materials)
and [Liquid Glass presentation](https://developer.apple.com/videos/play/wwdc2025/219/):
preserve backdrop color and depth while keeping foreground content legible.
An integer, background-aware dimming layer bounds bright backdrops; clear glass
retains more color than regular glass. Bilinear reconstruction removes coarse
steps from the downsampled blur. Directional rims, rounded corners and softer
shadows distinguish window depth, with translucent button hover/press feedback.
This is a CPU-rendered native interpretation, not Apple's implementation or a
claim of optical refraction/60 Hz animation.

The compositor uses about 27.4 MB of flat image space, including a full-resolution
packed background cache and pitched composition buffer. A bounded 480-row
snapshot scratch surface supports the largest current client viewport without
reserving another full display buffer. Source/destination strides and complete
lengths are checked by the safe row copier; host tests cover padding, guard words,
overflow and rejection without partial writes.

See [testing](testing.md) for native protocol/raster tests and the real GRUB
desktop, glass and performance checks. QMP assertions and screenshots supplement
each other; host GUI input remains a separate manual check.
See [executed red/green evidence](desktop-validation.md) for this change's results,
the keyboard-wakeup regression and remaining limits.


## Source organization

Each executable has a small entry point. Modules follow ownership rather than
sharing a desktop implementation between the two processes.

| Process/module | Owns |
| --- | --- |
| WM [entry point](../user_programs/wm/src/main.rs) | Runtime entry and service startup |
| WM [session](../user_programs/wm/src/session.rs) | Message dispatch, shell identity, work area, stacking, focus, input routing and client lifecycle |
| WM [window](../user_programs/wm/src/window.rs) | Per-client state, initial placement and bounded cache-slot allocation |
| WM [display](../user_programs/wm/src/display.rs) | Private framebuffer/cache storage, snapshots, composition, decorations, cursor and presentation |
| WM [damage](../user_programs/wm/src/damage.rs) / [glass](../user_programs/wm/src/glass.rs) | Independent tile tracking and material algorithms |
| DE [entry/event loop](../user_programs/de/src/main.rs) | Surface connection, initial frames and event coordination |
| DE [applications](../user_programs/de/src/applications.rs) | Discovery, executable naming, compatibility policy, launch and rollback |
| DE [view](../user_programs/de/src/view.rs) | Wallpaper/panel/launcher painting through the SDK canvas |
| DE [wallpaper](../user_programs/de/src/wallpaper.rs) | File loading, format validation and immutable image storage |

The session asks the display to snapshot a desktop frame or reset window content;
it cannot directly borrow the renderer's static buffers. The display depends on
window state and the SDK, without importing session coordination. Both programs
communicate solely through the existing SDK protocol. Application launch policy
and wallpaper file handling remain inside the DE.

Wallpaper storage is initialized once. An atomic claim prevents a second caller
from mutating storage after an immutable image slice has escaped; relaxed ordering
is sufficient because other callers only return failure and never read that storage.
This is not a general concurrent image cache. The compositor remains single threaded.
