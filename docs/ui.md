# Native window and UI toolkit

Applications use `userlib::ui` in Ring 3. The SDK owns layout, widgets and drawing;
the WM owns placement, stacking, focus policy, decorations and backdrop materials.
The kernel only protects the existing private surfaces and transports bounded
scalar messages. No widget, blur or theme policy executes in the kernel.

## A complete application

```rust
#![no_std]
#![no_main]

use userlib::{sys, ui::{Material, Window, WindowOptions}};

#[no_mangle]
pub extern "C" fn user_main() {
    let window = Window::create(
        WindowOptions::new("Settings")
            .content_size(640, 400)
            .background(Material::GlassClear),
    ).expect("create window");
    let mut enabled = false;
    window.run(|ui| {
        ui.heading("Appearance");
        ui.label("Your own app, on the native desktop.");
        if ui.button("Change appearance").clicked() {
            enabled = !enabled;
        }
        ui.card("PREFERENCE", if enabled { "Enabled" } else { "Disabled" });
    }).expect("window loop");
    sys::exit(0);
}
```

[Hello](../user_programs/hello/src/main.rs) is the runnable widget example.
[Portal](../user_programs/portal/src/main.rs) demonstrates custom canvas drawing:
`window.draw(|canvas| { ... })`, `ui::text`, `Color`, and `Rect`. It retains its shell
and keyboard handling without managing framebuffer pointers or pitch.

## Drawing and input contract

- Content dimensions are independent of desktop resolution: 160–640 pixels wide
  and 96–400 high, also bounded by the display's workspace (88 pixels are reserved
  vertically for desktop panels, placement margin and decorations). The WM chooses placement and adds a
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
  RELEASE, which the WM sends only after copying the submitted pixels to its own
  cache. Pixels stay unchanged while a submission is outstanding. This supplies
  coherent snapshots for SDK clients; it is not buffer swapping or display vsync.
  Raw clients remain responsible for following the same protocol.
- `poll_event` provides local pointer, focus and close events. The WM routes
  pointer events to the focused native window, retaining delivery outside its
  bounds so a pressed widget can cancel on release outside. Keyboard reads still
  use focused `sys::read_nb`; the toolkit does not provide text entry widgets yet.
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
151 KiB. The existing full-display back/snapshot buffers remain.

This is CPU-rendered frosted/clear glass, without GPU acceleration, optical
refraction, fluid transitions, dynamic resizing, or system accessibility settings.
The existing approximately 18.2 Hz PIT polling remains; this is not a claim of
60 Hz animation. Legacy Snake still uses the WM's historical scaling adapter.

## Window protocol version 1

See [the scalar channel ABI](syscalls.md#display-owner-message-channel).
The high nibble is the message kind; all payloads are integers, never Rust structs
or pointers. Sender IDs come from the kernel.

| Kind | Direction | Payload |
| --- | --- | --- |
| `1` CONFIGURE | App → WM | Width bits 0–9, height 10–18, material 19–20; other low bits zero |
| `2` TITLE | App → WM | Three ASCII bytes in bits 0–23; chunk index 24–27 (0–7); NUL pads a 24-byte title |
| `3` COMMIT | App → WM | 28-bit frame serial; do not modify the buffer until matching RELEASE |
| `4` RELEASE | WM → app | Matching frame serial, after snapshot copy |
| `5` POINTER | WM → app | Signed 11-bit x in 0–10, signed 11-bit y in 11–21, buttons 22–24; outside coordinates clamp to −1024…1023 |
| `6` FOCUS | WM → app | Bit 0 is focused |
| `7` CLOSE | Either | Window close/disconnect request |
| `8` ACCEPT | WM → app | Configuration accepted |
| `9` REJECT | WM → app | Invalid/unsupported configuration or no free slot |

Only the first configuration is accepted for a window. Geometry, content, and
material are fixed for its lifetime. Unknown message kinds are ignored. Configuration
and frame replies are retried before coalesced pointer motion; pending focus state
is also retried when a client inbox is full. This is a small project protocol,
not Wayland compatibility or a general capability/session system.

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
