# Google verification navigation and host-only document cookies

This follow-up fixes a concrete Google search failure in the native Ring 3
runtime. The generated script previously overflowed the parser stack before
executing; the page profile now permits128KiB within the existing adapter ceiling.
Page Date, URL metadata, navigator identity, a coarse monotonic performance clock,
host-only document cookies and bounded script navigation let the actual served
verification script produce a cookie and navigate back to Google.

**Google results still fail the public acceptance gate.** In the2026-11-06 local
test (2026-11-07 UTC), the default homepage's real form was edited to search for
`rust operating system`. The native log records script navigation, followed by
HTTP429, Google's unusual-traffic page, zero linked headings and one rendered
destination. That page also has one unsupported script; the usable default
homepage form still has two script errors. A full headless Chromium143
reference using its default identity reaches `/sorry/` with zero result headings. No CAPTCHA is solved or
service rejection bypassed. Browser API and layout gaps remain independently.

The [strict live failure](browser-google-navigation/google-results-failure.log)
is preserved. The [reference metadata](browser-google-navigation/chromium-reference.json)
separates the initial Chromium HTTP200 from its final `/sorry/` document. A200
challenge or a429 page must never count as search success. Raw Google responses,
cookie values, challenge tokens, URL nonces and the public-page screenshots are
kept out of the committed evidence; those screenshots contain transient tokens.

## Ownership and limits

The implementation is split between the pure
[cookie policy](../crates/browser/src/cookies.rs),
[copied context/effect codec](../crates/browser/src/page_context.rs),
[restricted page realm](../crates/browser-dom/src/lib.rs) and
[application loader](../user_programs/jbrowse/src/loader.rs).

- All changes are userspace. There are no new kernel syscalls, elevated browser
  privileges, dependency/toolchain updates or host-rendering components.
- The loader thread owns an in-memory cookie jar across ordinary navigations.
  Only cookies visible to the current document are copied into its restricted
  worker. The worker retains no network, filesystem or compositor authority.
- The page runtime has the same4MiB engine heap,100 interrupt polls,128 jobs per
  checkpoint,64 scripts,128KiB script-source aggregate and16MiB process heap.
  Only the page stack allowance changes from64KiB to128KiB. The plain evaluator
  keeps its original stack/clock-free profile.
- Date is an explicit engine opt-in. The native adapter uses the existing RTC
  syscall at one-second resolution and an explicitly UTC local timezone. An
  exact-guarded build-time offset patch leaves vendored QuickJS input unchanged;
  it does not pretend to implement libc localtime or a timezone database.
- `navigator.userAgent` matches the actual `RustOS-curl/1` HTTP identity and
  platform is `RustOS`. `cookieEnabled` reflects whether a page context exists.
  No Firefox identity is substituted. `performance.now()` uses PIT-derived
  milliseconds and never moves backwards; there are no fake timers or beacons.
- The URL subset supplies `document.URL`, `window/document.location`, href,
  protocol, host/hostname/port, pathname/search/hash, assign and replace.
  Setting location/href queues controller navigation. It does not implement
  complete WHATWG URL semantics, hash events or History API behavior.
- Cookies are host-only, with path matching, Secure, Max-Age and an IMF-fixdate
  Expires subset. Max-Age wins over Expires. Replacements/deletions and resource
  failures preserve the previous accepted value. Insecure documents cannot
  replace/delete overlapping secure cookies. `__Secure-`/`__Host-` restrictions
  are checked. The jar is bounded to32 entries and4096 aggregate header bytes;
  oversize writes are ignored without eviction. Values do not implement Debug.
- Domain and HttpOnly document assignments are rejected. There is no response
  Set-Cookie processing, public-suffix/domain-cookie support, SameSite model,
  persistent profile, partitioning or authenticated-site compatibility claim.
  This deliberately limited jar is not the completed P3/P5 storage subsystem.
- The HTTP SDK consults the application cookie policy for each effective redirect
  URL, rechecking UTC expiry/host/path/transport. Header controls/CRLF and lengths
  are validated before transmission. Existing APIs supply an empty cookie policy;
  TLS verification, cancellation, socket cleanup and POST redirect rules remain.
- A page navigation stops further scripts in the old document after its Promise
  checkpoint. The loader follows at most four script redirects as GETs, then
  reports an error and remains usable. HTTPS downgrades and non-HTTP(S) navigation
  fail both in the worker binding and at controller commit.

## Private protocol

`JBW1` adds operation5 (Page); existing Eval/Document operations are retained.
The complete input stays at128KiB, including context, and output at8192 bytes.
Controllers and workers must be rebuilt together.

`JPQ1` contains magic, a little-endian u64 UTC-second snapshot, then three
u32-length-prefixed UTF8 fields: URL (2048 bytes), cookie snapshot (4096 bytes),
and HTML. Cookie snapshots contain newline-separated host-only assignments with
path/Secure/remaining-lifetime metadata, preserving duplicate names at different
paths. They are private context, not raw HTTP headers.

`JPR1` contains magic, u32 cookie-assignment count (maximum32), a u32-length-prefixed
`JDM1` document record, zero or more u32-length-prefixed assignments (4096 bytes
each), then a u32-length-prefixed navigation target (2048 bytes; empty means none).
All fields together fit8192 bytes. Decoders reject invalid UTF8, truncation,
trailing data and resource violations. No pointers or OS handles enter the wire.
The controller revalidates cookie scope, URL policy and the entire DOM batch
before publishing any effect; forged/rejected replies cannot partially change
the visible document or jar.

## Tests and evidence

The deterministic fixture is independent of Google and contains no copied site
script. A48-level generated expression computes42; the page sets an expiring
cookie, requests Promise navigation, and attempts an out-of-scope cookie. The
server requires exactly `proof=42` before returning the actual title/snippet/link
page. A second navigation checks persistence and worker readback; a self-redirect
must fail after five fetches and then recover on a valid navigation.

[Before](browser-google-navigation/native-red.log), the native regression loaded
`Old document executed` after a script error instead of the verified destination.
[After](browser-google-navigation/native-context.log), framebuffer colors, exact
server headers, session persistence and redirect-loop recovery pass. These
[native pixels](browser-google-navigation/native-cookie-navigation.png) show the
local fixture, **not Google results**. The recovery assertion is in the native
context log. The full
[browser integration run](browser-google-navigation/native-integration.log) also
passes scripts, password/POST/303, attributes/base64, resize/maximize, PNG, fonts,
real CERN reading, cancellation, close and relaunch.

The checked-in `jbrowse-*-red.log` files record failing host regressions for the
parser stack, Date, cookie policy, context wire, realm APIs, per-hop HTTP cookies,
worker transport, atomic controller commit and visible linked-heading evidence.
The excess256-level expression still fails, and a subsequent script recovers;
execution/memory limits have not been relaxed to suppress an existing failure.

Current checks use the pinned nightly outside the bare-metal Cargo configuration:
50 browser tests (including the optional script prototype),12 DOM tests in each
of the libc/freestanding profiles, a separate native-allocation lifecycle test
(40 realms),7 plain and8 freestanding engine tests,6 worker protocol tests,
33 HTTP tests and57 stdlib-only Python tool tests pass. Complete command output
is retained alongside the red logs in `docs/browser-google-navigation/`.

The [worker/frame cleanup](browser-google-navigation/native-worker.log) and
[desktop integration](browser-google-navigation/native-desktop.log) checks pass
on8 CPUs. The [three-trial performance run](browser-google-navigation/performance.json)
passes the unchanged1/2/1-second budgets: maximum window0.901s, local first
page1.350s and navigation0.808s, alone in a1GiB,8-vCPU TCG guest at1920×1080.
Those are controlled local-page timings, not Google results latency or a manual
host-desktop measurement. The [validation record](browser-google-navigation/validation.json)
identifies source/binary hashes and the tested image.

Reproduce with a fresh disposable image:

```bash
./tools/build-grub-disk.sh target/jbrowse-google.img
RUSTOS_TEST_DISK=target/jbrowse-google.img RUSTOS_CPUS=8 RUSTOS_JBROWSE_PAGE_CONTEXT=1 python3 -B e2e/jbrowse.py
RUSTOS_TEST_DISK=target/jbrowse-google.img RUSTOS_CPUS=8 RUSTOS_JBROWSE_REQUIRE_GOOGLE_RESULTS=1 python3 -B e2e/jbrowse.py
```

The strict Google gate now requires HTTP200, no script/style errors, no truncated
layout, and at least three rendered linked headings/destinations. It still does
not verify the full P4 exit task: three queries, readable snippets, follow a real
result, Back and refine. That task remains unpassed. Events/timers, external/module
scripts, DOM construction, response cookies and large executable documents remain
gaps; CAPTCHA presentation and exact Firefox layout are unverified. YouTube video,
Facebook authentication and broad WPT/Test262 remain open. QMP assertions are not
a manual host GUI/input test.
