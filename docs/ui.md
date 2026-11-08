# Native window and UI toolkit

Applications use `userlib::ui` in Ring 3. The SDK owns layout, widgets and drawing;
the WM owns placement, stacking, focus policy, decorations and backdrop materials.
The kernel only protects the existing private surfaces and transports bounded
scalar messages. No widget, blur or theme policy executes in the kernel.

## A complete application

Use the [shared Cargo executable root](development.md#writing-an-application) for this application.

```rust
use userlib::{rsx, sys, ui::{Material, Style, View, Window, WindowOptions}};

#[derive(Default)]
struct State { enabled: bool }
#[derive(Clone)]
enum Action { Toggle }

fn main() {
    let window = Window::create(
        WindowOptions::new("Settings")
            .content_size(640, 400)
            .background(Material::GlassClear),
    ).expect("create window");
    window.mount(State::default(), app, update).expect("component loop");
    sys::exit(0);
}

fn app_style() -> Style { Style::new().padding(24).gap(8) }

fn app(state: &State) -> impl View<Action> {
    rsx! {
        column { style: app_style(),
            heading { "Appearance" }
            label { "Your own app, on the native desktop." }
            button { key: "toggle", onclick: Action::Toggle,
                if state.enabled { "Disable" } else { "Enable" }
            }
            { preference_card(state.enabled) }
        }
    }
}

fn preference_card(enabled: bool) -> impl View<Action> {
    rsx! {
        card { title: "PREFERENCE", if enabled { "Enabled" } else { "Disabled" } }
    }
}

fn update(state: &mut State, action: Action) {
    match action { Action::Toggle => state.enabled = !state.enabled }
}
```

[Hello](../user_programs/hello/src/main.rs) is the runnable widget example.
[Portal](../user_programs/portal/src/main.rs) demonstrates custom canvas drawing:
`window.draw(|canvas| { ... })`, `ui::text`, `Color`, and `Rect`. It retains its shell
and keyboard handling without managing framebuffer pointers or pitch.

## Components, styles and state

`rsx!` builds ordinary Rust values implementing `View<Message>`. Components are
functions returning views; `{ component(props) }` embeds another component. Keep
component construction pure. Put state changes and side effects in the update
handler passed to `mount`. Buttons declare a stable `key` and an `onclick` message;
changing a label does not change the widget's identity. Keys must be unique within
the mounted view.

A click is captured during rendering, then delivered once to `update`. The SDK
clears the input transition and renders the updated state before publishing the
frame, so text/style changes appear without a further mouse movement. Initial
render, hover and focus changes do not invoke update handlers. The mount loop
retains application state across frames and sleeps when idle.

Supported native syntax:

- `column { style: Style::new().padding(24).gap(8), ... }` lays out nested children
  vertically. `padding` and `gap` use pixels; `.color(Color::rgb(...))` inherits a
  foreground color through nested components. Style functions return typed values.
- `heading { "Title" }`, `label { "Text" }`, and
  `label { color: Color::rgb(0x9dd6ff), "Override" }` render sharp text.
- `button { key: "save", onclick: Action::Save, "Save" }` declares an action.
- `card { title: "STATUS", "Description" }` and `space { 12 }` provide presentation
  and spacing. `{ child(props) }` accepts any Rust expression returning a view.

Components use the existing typography and control sizing (heading slot 36px,
label 22px, button 50px including its 38px control, card 88px including its 72px
panel), with container gaps added between children. Excessive padding produces
zero available width; drawing and hit-testing stay clipped to window content.
Trees deeper than 16 levels return `Error::InvalidView` before drawing, protecting
the small user stack. The tree stores concrete nested values without allocation,
unsafe code or a DOM. This component layer has no CSS parser, React/Dioxus dependency,
hook system or reconciliation engine. Text is borrowed `&str`; the mounted root
must return a fixed view type whose text does not borrow transient application
state. The example uses static strings selected from state. Dynamic-length lists
and formatted owned text are not implemented by this first component layer.

The existing `Window::run`, `Ui` and custom canvas interfaces remain available.
The WM still controls materials and window policy; only the client SDK interprets
components, layout and styles. The window protocol and syscall ABI are unchanged.

## Drawing and input contract

- Content dimensions are independent of desktop resolution: 160–1920 pixels wide
  and 96–1080 high, also bounded by the display's negotiated workspace. The SDK
  preflights only the titlebar; the DE requests panel reservations and the WM
  rejects content that does not fit them. The default DE reserves 28 pixels above
  and 32 below, with a further 28 pixels for window decorations. The WM chooses placement and adds a
  28-pixel titlebar. Native clients use content-local coordinates.
- The current OS maps one private, display-sized backing surface per process.
  The SDK exposes one window creation per process, including after dropping a
  handle. It validates geometry and holds the sole safe drawing handle; it does
  not allocate new mappings or pretend these are independently allocated buffers.
- Each draw clears content to transparent. `Color::rgb` is opaque;
  `Color::rgba` accepts straight components and stores premultiplied ARGB.
  Canvas operations clip and preserve row padding. Existing XRGB applications
  remain opaque legacy clients.
- `draw` publishes a completed frame and sends COMMIT. The next draw waits for
  RELEASE, which the WM sends after copying submitted pixels to its cache and
  presenting the frame. Pixels stay unchanged while a submission is outstanding. This supplies
  coherent snapshots for SDK clients; it is not buffer swapping or display vsync.
  Raw clients remain responsible for following the same protocol.
- `poll_event` provides local pointer, focus, resize and close events. The WM routes
  pointer events to the focused native window, retaining delivery outside its
  bounds so a pressed widget can cancel on release outside. Keyboard reads still
  use focused `sys::read_nb`; the SDK also provides an independent bounded text editor model.
- `run` redraws on initial display and window input, and sleeps otherwise. Widget
  state lives in the app's closure. Buttons activate on release inside the widget
  that received the press; focus loss cancels capture. Button labels identify
  widgets and must be unique and stable in a view. Current buttons use pointer
  input; keyboard navigation and accessibility semantics remain future work.
- Creation and transport return explicit errors. Full message queues apply
  backpressure. SDK waits yield and time out after 36 PIT ticks (about two seconds).
  Close requests allow 18 ticks for graceful exit before the WM terminates an
  unresponsive client. Exited native windows release their WM cache slots.
  Existing kernel frame/stack reclamation gaps remain unchanged.

## Materials and rendering

`Opaque`, `Glass`, and `GlassClear` describe the background, not a blur algorithm.
The WM captures the already composed lower layers, including overlapping windows,
with a 16-pixel sampling margin, downsamples and runs a separable integer blur,
adds a dark readable tint, then composites sharp application content. Rounded
corners use subpixel coverage; the WM draws the edge, title and close control.

Any lower-layer content or layout change restores the desktop and recomposes the
window stack. A stationary glass window therefore updates when its backdrop
changes, without repeatedly filtering the previous glass result. Unchanged scenes
retain the composed buffer. During composition the WM checks pointer position at
bounded row intervals and updates only the cursor, preserving its underlying
display pixels separately. This keeps motion responsive without exposing the
unfinished scene. The final presentation still flushes dirty tiles.
Six content cache slots reserve 5.86 MiB; two small blur scratch arrays add about
151 KiB. The compositor also caches the DE's rendered full-display surface and
uses a bounded 480-row scratch area for client snapshots. See the
[two-process desktop protocol](desktop-environment.md).

This is CPU-rendered frosted/clear glass, without GPU acceleration, optical
refraction, fluid transitions, dynamic resizing, or system accessibility settings.
The existing approximately 18.2 Hz PIT polling remains; this is not a claim of
60 Hz animation. The DE requests a generic legacy scaling adapter for Snake;
the compositor contains no application-name exceptions.

## Window protocol version 1

See [the scalar channel ABI](syscalls.md#display-owner-message-channel).
The high nibble is the message kind; all payloads are integers, never Rust structs
or pointers. Sender IDs come from the kernel.

| Kind | Direction | Payload |
| --- | --- | --- |
| `1` CONFIGURE | App → WM | Width bits 0–9, height 10–18, material 19–20; other low bits zero |
| `2` TITLE | App → WM | Three ASCII bytes in bits 0–23; chunk index 24–27 (0–7); NUL pads a 24-byte title |
| `3` COMMIT | App → WM | 28-bit frame serial; do not modify the buffer until matching RELEASE |
| `4` RELEASE | WM → app | Matching frame serial, after snapshot copy and presentation |
| `5` POINTER | WM → app | Signed 11-bit x in 0–10, signed 11-bit y in 11–21, buttons 22–24; outside coordinates clamp to −1024…1023 |
| `6` FOCUS | WM → app | Bit 0 is focused |
| `7` CLOSE | Either | Window close/disconnect request |
| `8` ACCEPT | WM → app | Configuration accepted |
| `9` REJECT | WM → app | Invalid/unsupported configuration or no free slot |

Kinds A–E implement the separate DE's background role, workspace reservations,
legacy window registration and full-display pointer routing. See the
[shell extension](desktop-environment.md#shell-message-extension). Existing app
message kinds and syscall numbers are preserved.

Only the first initial configuration is accepted for a window. Version-1 geometry
and material remain fixed; version2 adds compositor-offered resizing as described below. Unknown message kinds are ignored. Configuration
replies precede pointer delivery; frame releases follow presentation. Pending replies
and focus state are retried when a client inbox is full. Button transitions sampled
during rendering are queued in order (32 entries); motion coalesces to its latest
position. Overflow retains the oldest edges and reconciles to the newest state. This is a small project protocol,
not Wayland compatibility or a general capability/session system.

## Window protocol version 2 and resizing

The compositor owns resize/move/maximize/restore and all decoration hit testing.
The DE continues to reserve its panels. Drag any edge/corner of a new SDK window;
use the square control immediately left of Close to expand to the workspace and
restore its previous bounds. Maximized windows retain the titlebar and DE panels.
Their edges/titlebar do not start another resize/drag until restored. Interactive
minimum content is the lesser of the initial size and320×200; initially smaller
supported clients retain their smaller minimum. Raw and version-1 clients keep
fixed dimensions and their existing pointer/commit format.

New SDK configurations set bit27 to select version2. Initial width/height still
fit the complete mapped display surface, including the28-pixel titlebar and DE
work-area constraints. Width and height use11 bits each. No syscall changes or
additional surface mappings are needed; snapshots/canvases retain the actual
framebuffer pitch. All clients must keep a committed surface immutable until
matching RELEASE, including while a resize offer is pending.

| Kind | Version-2 payload |
| --- | --- |
| CONFIGURE1 | Width0–10, height11–21, material22–23, version flag27; other bits zero |
| COMMIT3 | Actual submitted width0–10, height11–21, serial22–27 (six bits) |
| RELEASE4 | Exact28-bit COMMIT payload, after copying/presenting that frame |
| POINTER5 | Signed12-bit x0–11/y12–23, buttons24–26; bit27 zero |
| RESIZE F | WM→app only: offered width0–10/height11–21; other low bits zero |

Only one resize offer is active per client; further drag targets coalesce. The
WM preallocates staging buffers before publishing an offer, keeps the old cache
and geometry for old-size frames, and adopts the offered bounds on the first
matching new-size frame. Unoffered frame sizes cannot reinterpret a previous
submission. Frame dimensions travel with each commit, so a resize arriving during
an internal RELEASE wait cannot race an old frame. The SDK validates offers
against its original full surface, cancels widget capture, and emits
`Event::Resized { width, height }`. `Window::width/height` and the next Canvas use
the new size; generic component loops redraw automatically. Custom applications
handle the resize event or read canvas dimensions while drawing.

Decoration pointer capture stays in the WM through release. A maximize click
must not become a client click after the window moves underneath it. jbrowse
retains its document and edited form values, reflows using the existing shared
font metrics, and anchors URL/scroll/status controls to the new canvas. Other
custom applications retain their own content-layout policy; Terminal currently
keeps its fixed text grid within the resized canvas.

The compositor owns per-slot packed content/backdrop Vec buffers with checked
geometry and fallible reservation. At most six live slots each retain a front
and one staged cache; removal releases both. Maximum supported cache payload is
under193 MiB across all six full-size front/staging pairs, plus display/scratch
buffers. The normal workspace is smaller. Allocator high-water heap pages are
reused, not promised to return immediately to the OS. A failed resize allocation
keeps the old window; no incomplete buffer is offered. Full-display scratch and
blur sizes are derived from the existing1920×1080/pitch8192 platform envelope.

See [resize evidence and limits](window-resize.md). Version-1 wire handling above
is retained for older binaries; all repository applications/metadata were rebuilt.

## Validation

See [executed validation and remaining limits](ui-validation.md) for the red/green
results and screenshots from this change.

Run the host raster/input/protocol and blur tests, rebuild all apps and the disk,
then run `e2e/glass_ui.py`, `e2e/desktop.py`, and `e2e/desktop_perf.py` as listed in
[testing](testing.md). The new interaction test checks negotiated geometry,
rounded/translucent pixels, sharp text, button capture, drag restoration, live
lower-layer damage, focus, and close/reopen. Kernel display-mapping tests cover
authenticated FIFO delivery, queue bounds, isolation and dead recipients.
Screenshots are generated from these QEMU runs. Manual host GUI testing remains
separate from QMP-injected input.

## Text entry and contained viewports

`ui::text_input::TextInput` (alloc feature) owns at most 2048 bytes of single-line
UTF-8 text. It supports selection replacement, printable ASCII byte input,
UTF-8-safe Backspace, and home/end control bytes. Applications own focus,
selection policy, rendering and transport-specific key decoding. The current
keyboard syscall does not deliver arbitrary raw-key navigation or Ctrl shortcuts.
Clicking the [browser](jbrowse.md) address selects all; Escape also selects it.

`Canvas::viewport(Rect)` returns a borrowed canvas only for a nonempty fully
contained rectangle. Coordinates become local, scanline pitch is preserved,
and drawing clips within the rectangle. The borrow prevents simultaneous mutable
access through the parent, and no new pointer or unsafe code is needed. Browser
scrolling uses this to prevent page text from painting over native chrome.

`Canvas::region_digest(Rect)` fingerprints a fully contained RGB24 region without
mutating it; alpha and row padding are excluded. This noncryptographic developer
readback helper returns `None` for invalid geometry. It does not acknowledge
presentation. The [browser probe](browser-compatibility-p8.md) waits for a matching
physical framebuffer region separately and excludes compositor decorations.

Input received while waiting for frame RELEASE is buffered in a bounded FIFO and
returned by `poll_event` in order. Pointer press/release and focus transitions
are preserved. Capacity is 64 events; overflow returns `Error::Unavailable`
rather than silently dropping a transition.

## Optional proportional text and image painting

Enable the SDK `fonts` feature for `ui::font::{FontSet, Painter, Style, Family}`.
Load trusted font bytes once, share immutable metrics with layout and keep each
Painter/cache on its painting thread. The coverage cache is bounded to512 glyphs
and1 MiB. Embedded browser faces and known limits are documented in
[P2 presentation](browser-rendering-p2.md). Other apps keep the bitmap path.

`Canvas::alpha_mask` blends a complete grayscale mask; `rgba_image` paints a
complete RGBA buffer into a destination rectangle with nearest-neighbor scaling.
Both validate checked buffer geometry before drawing, return false on malformed
input, clip to the current viewport and preserve row padding. They allocate no
buffers and interpret no URLs/codecs. Zero destination extent is a successful
no-op for an otherwise valid image. The caller owns decoding/resource budgets.
