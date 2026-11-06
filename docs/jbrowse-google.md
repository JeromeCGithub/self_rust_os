# Google startup and HTML search forms

jbrowse now opens `https://www.google.com/` automatically when launched without
a URL. An explicit first argument still overrides the start page. The real Google
homepage was fetched over the guest's verified HTTPS stack and its search field
was edited and submitted in QEMU.

**Usable Google results remain incomplete.** The tested search response is a
JavaScript-gated page. The current bounded interpreter cannot execute it; jbrowse
shows an explicit unsupported-JavaScript page instead of a blank document.
This is not a successful Google search-results acceptance pass. No results are
fabricated, no alternative search provider is substituted, and no host browser
or proxy renders content for the guest.

The [homepage screenshot](screenshots/jbrowse-google.png) shows Google's actual
HTML with native form controls. The [search response screenshot](screenshots/jbrowse-google-response.png)
shows the remaining limitation. The [local GET-search screenshot](screenshots/jbrowse-form-search.png)
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
- POST and other unsupported methods return an explicit error. Actions accept
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

This remains a limited native browser. Form rendering is simple block/inline
flow, not Google's exact layout. Image logos use alt text. File/password/radio/
checkbox controls, selects, arbitrary event handlers, full form ownership rules,
legacy form encodings and POST are not implemented. Native typing currently
follows the SDK's ASCII keyboard transport; UTF-8 form values still encode safely.

## Reproduction and acceptance status

```bash
./tools/build-grub-disk.sh target/jbrowse-google.img
RUSTOS_TEST_DISK=target/jbrowse-google.img RUSTOS_JBROWSE_GOOGLE=1 RUSTOS_JBROWSE_SCRIPTS=1 python3 -B e2e/jbrowse.py
# Strict goal check: remains failing on the public JavaScript-gated response.
RUSTOS_TEST_DISK=target/jbrowse-google.img RUSTOS_JBROWSE_GOOGLE=1 RUSTOS_JBROWSE_REQUIRE_GOOGLE_RESULTS=1 python3 -B e2e/jbrowse.py
```

The ordinary fixture path supplies an explicit local initial URL, so it does not
depend on Google's availability. Google mode launches without an argument,
waits for the real rendered form, types `rust operating system`, submits it and
captures the response. The strict mode additionally rejects the unsupported-script
fallback and must not be presented as green until usable results actually work.

The local fixture asserts the actual received query for Enter and button
submission, rendered response pixels, and recovery. Navigation waits use a fresh
log position, preventing an old page with the same title from satisfying a check.
Public testing is separate from this deterministic coverage.

Red evidence: four form/encoding cases initially failed, the charset observer
reported None instead of Windows1252, Google did not load automatically, and
`#fff` produced `0x0fffff` instead of white. These now pass. The strict real Google
results check remains a disclosed failure, with the exact message
`Google results remain blocked by unsupported JavaScript`.

Host tests run with the pinned nightly outside the root bare-metal Cargo config:

```bash
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/browser/Cargo.toml --target x86_64-unknown-linux-gnu --features javascript
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/http/Cargo.toml --target x86_64-unknown-linux-gnu
```

See [the JavaScript prototype and engine research](jbrowse-javascript.md) for the
missing fuller engine port and DOM support. Resolving that runtime gap and
retesting the real results page are required before claiming the Google search
goal is complete. Manual host GUI checks also remain outstanding.
