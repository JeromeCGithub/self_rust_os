# jbrowse: a native single-tab browser

[jbrowse](../user_programs/jbrowse/src/main.rs) is an ordinary Ring 3 application.
Launch **JBROWSE** on the desktop, or run `./jbrowse` in Terminal.
An optional first argument supplies the initial URL:

```text
./jbrowse http://info.cern.ch/hypertext/WWW/TheProject.html
```

Click the address field to select its contents, type a replacement and press
Enter or click Go. An explicit HTTP/HTTPS URL navigates directly; a bare hostname
uses HTTPS; words become a percent-encoded Google search URL. This submits a
search, but does not promise that the search engine's modern result page fits
the supported HTML/CSS subset. R reloads the current document. Up/Down scroll;
click the document and use j/k or Space to scroll with the keyboard. Escape
selects the address again. Backspace edits it. There is one tab and no history.

## A real website to try

Use [CERN's original World Wide Web project](http://info.cern.ch/hypertext/WWW/TheProject.html).
The exact URL above was loaded through the guest's HTTP stack during this change.
It provides headings, paragraphs and relative hyperlinks without requiring
JavaScript. Internet/DNS availability is still a host prerequisite.

The local automated fixture independently verifies CSS backgrounds, linked
stylesheets and navigation, without relying on a public site staying available.
The [HTML/CSS screenshot](screenshots/jbrowse-html-css.png) and
[CERN screenshot](screenshots/jbrowse-cern.png) come from the tested QEMU build.

## Ownership and architecture

- `user_programs/jbrowse` owns navigation, address/search policy, bounded resource
  fetching, optional stylesheet fallback, scroll state, and painting.
- `crates/browser` is a host-testable `no_std + alloc` HTML/CSS/URL/layout library.
  It knows no syscalls, framebuffer addresses, sockets or kernel internals.
  DOM nodes use arena indices; layout returns owned paint and link hit records.
- `userlib::ui::text_input::TextInput` supplies bounded single-line editing.
  `Canvas::viewport` safely borrows a contained rectangle with local coordinates,
  retains pitch, and clips all drawing to protect browser chrome. Window events
  received during a frame ACK wait are retained in a 64-event FIFO instead of
  being lost; overflow returns an explicit transport error.
- `userlib::http::get_with_url` reports the effective response URL after redirects
  so relative resources use the response's location. Existing GET APIs are retained.
  The observer runs once after success and socket close; it does not run on failure.
- One native network worker receives a single replaceable navigation request.
  A generation number rejects stale results. App locks are held only when moving
  requests/results, never during network I/O or drawing. The main thread owns the
  window and keeps processing keyboard, pointer and close events during downloads.

No syscall numbers or privileged implementation changed. Existing socket, TLS
time/entropy, thread, event-wait and protected surface mechanisms are sufficient.
The kernel continues to validate handles, user buffers and process ownership;
parsing, resource policy and rendering remain entirely in Ring 3. Returning from
main exits the process and stops its worker; request sockets close on success
and failure through the existing HTTP transport.

## Supported subset and limits

HTML supports UTF-8 text, numeric/common named entities, titles, headings,
paragraphs, lists, block/inline containers, preformatted text, line breaks,
rules, image alt text, anchors, inline style and linked stylesheets. Omitted
body tags receive an implied body. This is a recovering subset parser, not the
HTML5 tree construction algorithm. Optional li and dt/dd end tags are recovered
within their list scope.

CSS supports type, class, ID, compound and descendant selectors, comma lists,
source order/specificity, inline declarations, inherited color/font size,
RGB hex/basic named colors, solid backgrounds, integer pixel font sizes,
uniform margin/padding, width, display:block/inline/none and preformatted
whitespace. Linked sheets are appended after embedded styles in link order.
Unsupported properties/selectors and at-rules are ignored; external sheet errors
fall back to readable unstyled HTML. Fonts use the existing bitmap UI renderer.

Documents are limited to 256 KiB, 4096 arena nodes (including an implied body),
64 open elements and 64 attributes per element. CSS has a 128 KiB aggregate
budget, 512 rules and four linked sheets of at most 32 KiB each. Layout limits
paint records to 16384, widths to 16384 pixels, and reports truncation. Address
input and resolved URLs are bounded to 2048 bytes. HTTP uses existing redirect
and connection deadlines and verified TLS; HTTPS downgrades are refused by the
transport. Network failure leaves the previous page readable and shows an error.

There is no image decoding, forms, cookies, storage, downloads, tabs/history,
flex/grid, responsive media queries, or browser-compatible font metrics.
The [inline scripting prototype](jbrowse-javascript.md) adds a bounded JavaScript
subset; it does not support modern JavaScript applications. Only HTTP(S) navigation is accepted; file/data/javascript links
are rejected. HTTPS capability does not imply support for arbitrary modern sites.
UTF-8 is required; legacy character encodings are rejected. Allocation failure
can still terminate the application through the SDK allocator. This is an early
browser, with no claim of production browser security or web standards conformance.

## Validation and reproduction

From outside the root's bare-metal Cargo configuration:

```bash
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/browser/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/http/Cargo.toml --target x86_64-unknown-linux-gnu
```

From the browser worktree:

```bash
rustc --edition=2021 --test user_programs/userlib/src/ui/host_tests.rs -o /tmp/jbrowse-ui-tests
/tmp/jbrowse-ui-tests
./tools/build-grub-disk.sh target/jbrowse.img
RUSTOS_TEST_DISK=target/jbrowse.img python3 -B e2e/jbrowse.py
RUSTOS_TEST_DISK=target/jbrowse.img RUSTOS_JBROWSE_PUBLIC=1 python3 -B e2e/jbrowse.py
RUSTOS_TEST_DISK=target/jbrowse.img python3 -B e2e/desktop.py
RUSTOS_TEST_DISK=target/jbrowse.img python3 -B e2e/tls.py
```

The browser runner uses a private image, an ephemeral local HTTP server and unique
QMP sockets. It asserts CSS pixels, redirect-relative stylesheet/link requests,
URL replacement, scrolling, invalid schemes, oversized response recovery,
terminal survival, close and relaunch. Logs, request traces and screenshots are
saved under `target/jbrowse-e2e/`. Optional public testing adds a CERN load and
waits for the new page's framebuffer before taking its screenshot.

Red evidence observed during this change: five initial engine cases failed on
the missing parser/URL behavior; text input failed two editing assertions; the
viewport test could not create its requested view; the HTTP observer saw no final
URL; QEMU reached Forge but failed to spawn JBROWSE. The first rendering run also
exposed omitted-body CSS, reproduced red as a white background instead of
`#f5f0df`, then corrected. The full desktop initially targeted the new JBROWSE
launcher at Portal's old coordinate; it now resolves launcher positions by name
from the same FAT order as DE, preserving the interaction assertions. This helper
is shared with the HTTP/TLS and performance launchers. A subsequent input-recovery
run exposed pointer events consumed by the SDK during frame ACK waits; two red
FIFO tests now cover press/release/focus/close ordering and explicit overflow.
A desktop argument check also found a correct Hello value interleaved with Portal
serial output; Hello now emits each argument diagnostic in one write so the
existing assertion can observe an intact record. No assertion or timeout was
weakened.

See the PR for the exact executed green results. Automated QMP injection is not
manual host GUI verification. A human still needs to check display/input capture
on their actual host, typography/readability and broader site compatibility.
