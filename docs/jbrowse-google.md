# Google startup and HTML search forms

jbrowse now opens `https://www.google.com/` automatically when launched without
a URL. An explicit first argument still overrides the start page. The real Google
homepage was fetched over the guest's verified HTTPS stack and its search field
was edited and submitted in QEMU.

**Usable Google results remain incomplete.** The [navigation/cookie follow-up](browser-google-navigation.md)
fixes a generated-script stack overflow and adds page URL, clock, host-only cookie
and script navigation support. The actual native search now runs Google's
verification redirect, then receives HTTP429 with an unusual-traffic page. A full
Chromium reference also reaches `/sorry/`. This is a failed results gate. There
are still missing browser APIs, including the CAPTCHA page's scripts/events.
No results are fabricated, no alternative provider is substituted, and no host
browser renders content for the guest.

The [homepage screenshot](screenshots/jbrowse-google.png) shows Google's actual
HTML with native form controls. The [search response screenshot](screenshots/jbrowse-google-response.png)
records the earlier unsupported-JavaScript limitation. The [local GET-search screenshot](screenshots/jbrowse-form-search.png)
is an independent fixture, clearly separate from Google results.

## Generic implementation

- HTML GET forms, text/search input, hidden fields and submit controls are parsed
  into bounded userspace models. Fields are edited with the shared SDK text editor
  and drawn/hit-tested in the document viewport. Enter submits the focused form;
  a button includes only its activated submitter.
- Submission resolves the action against the final document URL, preserves the
  fragment, replaces the action query, URL-encodes names/values, and excludes
  disabled/unnamed controls. Duplicate names retain their order. Readonly text is
  serialized but cannot be edited.
- GET and bounded URL-encoded POST are supported (see [P5 forms](browser-sessions-p5.md)); other methods return an explicit error. Actions accept
  only HTTP(S). Forms are capped at 16 per document and 64 fields per form;
  names, values, labels and actions have 2048-byte limits, as does the final URL.
- HTTP response metadata now includes a typed declared charset. The browser
  supports UTF-8 and Windows-1252 (including ISO-8859-1/Latin-1 labels); other
  declared encodings are rejected. Without a declaration, valid UTF-8 is preferred
  and invalid bytes use Windows-1252. This is a small decoding policy, not complete
  HTML encoding sniffing. Google's ISO-8859-1 response motivated the extension.
- Three-digit hex CSS colors now expand correctly, such as `#fff` to `#ffffff`.
  A failing regression reproduced the cyan Google background before correction.
- A document with script errors and no readable text or controls gets an explicit
  unsupported-script fallback. Existing visible content survives script failures.

These mechanisms are in the browser engine, application and Ring 3 HTTP SDK.
There is no Google-specific syscall, privileged renderer, or host-side fetch path
in the application. No kernel source or syscall ABI changed.

This remains a limited native browser. CSS layout differs from Firefox; see the
[measured comparisons](browser-css-comparison.md). Static PNG and proportional
fonts are supported; password fields and POST are supported by P5. Radio/checkbox
controls, selects, arbitrary event handlers, full form ownership rules and legacy
form encodings remain incomplete. Native typing follows the SDK's ASCII keyboard
transport; UTF-8 form values still encode safely.

## Reproduction and acceptance status

```bash
./tools/build-grub-disk.sh target/jbrowse-google.img
RUSTOS_TEST_DISK=target/jbrowse-google.img RUSTOS_JBROWSE_GOOGLE=1 RUSTOS_JBROWSE_SCRIPTS=1 python3 -B e2e/jbrowse.py
# Strict goal check: remains failing on the public unusual-traffic response.
RUSTOS_TEST_DISK=target/jbrowse-google.img RUSTOS_JBROWSE_GOOGLE=1 RUSTOS_JBROWSE_REQUIRE_GOOGLE_RESULTS=1 python3 -B e2e/jbrowse.py
```

The ordinary fixture path supplies an explicit local initial URL, so it does not
depend on Google's availability. Google mode launches without an argument,
waits for the real rendered form, types `rust operating system`, submits it and
captures the response. The strict mode rejects the unsupported-script fallback, non-200 responses,
script/style errors, truncated layout and fewer than three rendered linked
headings/destinations. This is still a minimum results detector, not the complete
three-query/follow/Back/refinement acceptance gate.

The local fixture asserts the actual received query for Enter and button
submission, rendered response pixels, and recovery. Navigation waits use a fresh
log position, preventing an old page with the same title from satisfying a check.
Public testing is separate from this deterministic coverage.

Red evidence: four form/encoding cases initially failed, the charset observer
reported None instead of Windows1252, Google did not load automatically, and
`#fff` produced `0x0fffff` instead of white. These now pass. The strict real Google
results check remains a disclosed failure. The current rejection is
`Google did not display a usable result list`, with HTTP429 and zero linked
headings; earlier evidence retains the unsupported-JavaScript failure.

Host tests run with the pinned nightly outside the root bare-metal Cargo config:

```bash
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/browser/Cargo.toml --target x86_64-unknown-linux-gnu --features javascript
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/http/Cargo.toml --target x86_64-unknown-linux-gnu
```

See the [current navigation/cookie evidence](browser-google-navigation.md) and
[complete roadmap](browser-support-plan.md) for the remaining work. Manual host
GUI checks remain outstanding.
