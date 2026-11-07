# Browser CSS comparisons and larger documents

This follow-up fixes several visible differences in Google's served homepage:
stacked header links, a left-aligned logo, full-width inline-block backgrounds,
ignored spacing/control dimensions and bitmap form text. It implements general
CSS behavior in Ring3. There is no Google-specific renderer, saved result page,
host rendering service or kernel browser policy.

**The complete user goal remains open.** Google results still need web APIs;
YouTube still needs executable-page compatibility and a video pipeline. The normal
modern Google page served to Firefox is not yet rendered equivalently by jbrowse.
P2/P4/P6/P7/P8 are not declared complete by these fixes.

## Actual reference browsers

Reference captures used Firefox159.0 and Chromium143.0.7499.169 (installed Brave),
with640×308 content. The checked-in original
[HTML fixture](../crates/browser/tests/fixtures/css-layout.html) isolates geometry
from site changes. Chromium measured the boxes below; host layout tests and QEMU
framebuffer assertions independently require those bounds. Native checks exclude
only the compositor's one-pixel side rims, with a regression rejecting other shifts.
The [reference record](browser-css/chromium-fixture.json) includes browser version,
viewport, computed properties and a SHA256 of the fixture.

| Box | x | y | width | height |
| --- | --- | --- | --- | --- |
| Padded panel |8 |4 |120 |32 |
| Auto-centered block |280 |40 |80 |30 |
| First centered inline block |230 |70 |80 |24 |
| Second inline block |310 |70 |100 |24 |
| Short centered flex child |530 |114 |40 |20 |
| Tall centered flex child |580 |109 |60 |30 |

[Chromium fixture image](browser-css/chromium-fixture.png) and native CSS evidence
are supplemented by executable assertions, not accepted merely from screenshots.

There are two different Google comparisons. A public response obtained using the
actual HTTP client identity, `RustOS-curl/1`, contains legacy table/center markup.
Rendering that captured response in
[Firefox](browser-css/firefox-same-markup.png) and
[Chromium](browser-css/chromium-same-markup.png) exposes jbrowse's layout defects.
The separate [normal Firefox request](browser-css/firefox-live-google.png) receives
a modern page with a pill-shaped search area. That is a different served document,
not proof that changing only jbrowse's colors would produce parity. The same-markup
Firefox capture transcodes the declared Latin1 response to UTF8 and adds a base URL
for its local-file resources. Native public requests are independent anonymous
requests, not claimed byte-identical snapshots. Full HTML/cookies/tokens are not
committed. No browser identity spoof was added to the product.

## Implemented layout and painting

CSS now retains four-sided spacing and longhands, horizontal auto margins,
content/border-box sizes and minimums, font shorthand/relative sizes/line height,
text alignment/decoration, inline blocks, simple flex rows with gap/justification/
cross-axis alignment, and rounded solid backgrounds. Fixed-height flex alignment
also handles an overflowing margin box without growing the declared container.
Invalid multi-side declarations leave all previous sides intact; a later valid
`display` overrides an earlier `display:none`.

Independent inline/flex formatting contexts live in
[formatting.rs](../crates/browser/src/layout/formatting.rs). They retain ranges
into the same bounded paint/control arenas. Moving a fragment translates pixels
and hit targets together. The box model separates margin, padding and content;
background radius and decoration reach the native painter. Padded linked inline
blocks retain clickable padding.

Controls carry their computed font, text color, padding, border and dimensions.
The application paints with the same proportional metrics used by layout; cursor
placement and password masks follow those metrics. A title/ARIA name is no longer
painted as a placeholder; only the actual placeholder is shown. Native control skin
and installed font substitution can still differ from Firefox.

This remains a subset: no complete flex grow/shrink/wrap/column algorithm, grid,
responsive media evaluation, positioning/stacking, percentage box layout, SVG,
shaping/bidi/web fonts or full inline-fragment/background/baseline behavior.
Simple table rows use bounded horizontal formatting, not the full CSS table sizing
algorithm. Legacy inline backgrounds retain their previous limited behavior; the
native scripted-class regression caught a lost inline background height during the
refactor, and the fix restores it without weakening that existing assertion.

Sizing/alignment references checked2026-11-06:
[CSS sizing](https://www.w3.org/TR/css-sizing-3/#box-sizing),
[flex alignment](https://www.w3.org/TR/css-flexbox-1/#justify-content-property).
The implementation and measured tests, not those full specifications, define the
supported subset.

## Document and worker budgets

The decoded HTML envelope is now2 MiB instead of256 KiB. The complete over-limit
boundary is still rejected.4096 nodes,64 nesting levels/attributes,128 KiB CSS,
512 rules,32 KiB per external sheet and16384 paint records retain their limits.
The linked-sheet count is now16 instead of four, with exact16/17 boundary tests
and native six-sheet source-order assertions.
The old oversized-download fixture is now2 MiB plus one byte: its purpose remains
rejection/recovery beyond the supported envelope, rather than rejecting a newly
supported256 KiB page.

Raw script/style scanning no longer lowercases/copies the entire remaining source
for every raw-text element. It scans to a complete case-insensitive end tag;
`</scripture>` does not terminate a script. App and native realm share script
classification. Non-executable data blocks do not start a worker solely because
they contain large data. A1.3 MiB native data-page fixture now paints in2.140s;
its old image failed the256 KiB fetch limit. The final rebuild repeats it in2.136s.
Missing optional sheets now produce a visible count and developer counter. A
native404 test verifies inline fallback painting and recovery on the next six-sheet
navigation. Load probes reject nonzero stylesheet-error counts.

Executable-document worker input remains128 KiB; the loader explicitly reports
that separate budget before spawning for an oversized executable document.
The engine still has4 MiB allocation/64 KiB stack/100 interrupt polls,64 scripts/
128 KiB aggregate script source,128 jobs/checkpoint,8192 result bytes, a16 MiB
process heap and180-tick transport deadline. Increasing HTML size does not relax
those execution limits or add missing browser APIs. The sampled real YouTube
response was1,315,215 bytes; the Google results response was93,330 bytes. Their
public acceptance is recorded separately from this local fixture.

The trusted worker protocol now uses up to three copied bytes per existing stdio
word. `Endpoint::send_slice` supports partial progress/backpressure with validation
and cancellation between bounded chunks; byte endpoints retain a default adapter.
Header/body packet boundaries stay separate. New explicit SDK packet APIs validate
canonical counts/padding; ordinary byte stdin remains unchanged. No kernel syscall
number, pointer interface, authority or queue capacity changed. A maximum131072-byte
eval succeeds three times alongside fault/deadline/malformed/recovery tests.

A first maximum-input test allocated its parent String after the frame baseline;
that falsely counted the parent's retained heap as worker leakage. The corrected
fixture owns that allocation before taking the baseline. The exact before/after
frame and break assertions were preserved. The
[observation](browser-css/parent-baseline-observation.log) remains visible.

## Executed checks and remaining acceptance

Red evidence includes missing layout/control APIs, all five initial wrong geometry
cases, minimum/display/title/radius/overflow/link regressions, missing reference/
readback/packet APIs, the old native CSS rectangles, a large document rejected at
256 KiB, a raw-text prefix bug and absent maximum-input transfer assertions.
The [evidence directory](browser-css/README.md) maps logs to these observations.

Host checks on this follow-up:43 browser tests including JavaScript,7 native DOM
cases,5 protocol cases, the packet codec test and57 tooling cases. No tests were
skipped, no watchdogs/budgets increased. The earlier28 Canvas tests are inherited
P8 evidence, not rerun for these changes. All apps and paired binary/metadata were
rebuilt with the pinned toolchain and a fresh release GRUB image. The test-only
maximum-input fixture was rebuilt/installed in disposable images after its baseline
correction. Kernel behavior is unchanged; kernel/audio/HTTP range results from
P6/P7 are inherited, not newly claimed here.

The latest isolated three-trial [performance result](browser-css/performance.json)
passes the original1s/2s/1s budgets: maxima0.955s window,1.416s local page,
0.843s navigation. Profile:1GiB,8CPU TCG,1920×1080, independent local HTTP/CSS,
no competing VM during timing. Public network timing is not a deterministic bound.

Further native integration and final public observations are recorded in the
[evidence index](browser-css/README.md). Manual host GUI input, host-speaker
listening, normal modern-Google parity, usable Google results, five-minute YouTube
video/audio and authorized Facebook session workflows remain unverified or failed.
The known bootloader unused rustc-abi warning remains.

## Final native observations

The final image digest is recorded in every
[latest probe](browser-css/latest/cern/probe.json). Native CSS bounds and field
ink pass: the18px field paints63×13 dark pixels, excluding the old8px bitmap
path. [Native fixture](browser-css/latest/css/body.png) versus the
[Chromium fixture](browser-css/chromium-fixture.png) supplies independent pixel
evidence for all six rectangles. The linked-sheet fixture loads all six sources
in order and paints the final sheet's color.

| Actual page | Final displayed-body observation | Goal status |
| --- | --- | --- |
| CERN |3.472s,25 laid-out links, no script/style errors | Narrow opening-document reading task passes |
| Google home |4.885s, centered logo, horizontal header, rounded sign-in, CSS-sized empty field; two script errors | Homepage layout improved; modern Firefox parity incomplete |
| Google query |4.300s, three script errors, zero result links | Search fails |
| YouTube watch |6.533s, full document/title fetched;128 KiB executable-document budget and two stylesheet failures | No video; fails5s load and playback acceptance |

Script failures are currently aggregated counts, not individual engine/API error
classifications. The Google probe gets past document transport and parsing; it
does not prove that every remaining failure is an API exception rather than an
execution-resource limit. Those must be diagnosed before tuning engine budgets.

[Before native homepage](browser-css/jbrowse-home-before.png),
[after native homepage](browser-css/jbrowse-home-after.png),
[after body crop](browser-css/jbrowse-home-after-body.png). Other latest images are
exact640×308 body crops from QEMU, with JSON/serial alongside them. Their inner
638×308 RGB digests were independently checked after copying. Original full frames
remain in the ignored `target/final-sites-16` artifacts. Cropping changes no pixels.

The [final corpus assessment](browser-css/corpus-assessment.json) intentionally
exits1: one partial run,1/27 ordinary tasks, no mandatory site pass. The
[observations](browser-css/corpus-report.json) retain every unrun task. The actual
default-Google form submission was also executed and fails the strict usable-results
assertion; see [its log](browser-css/google-form-gate.log). No homepage, HTTP200,
thumbnail or another search provider is substituted for the advertised tasks.

Full native integration on the final image passes restricted scripts, POST/password
forms, attributes/base64, maximize/restore/resize, PNG/fonts/Unicode, stalled HTTP/TLS
cancellation, navigation/error recovery, close and relaunch. Final worker runs pass
at1,8 and32 CPUs, including three full128 KiB inputs each and exact cleanup. The
desktop regression passed after the shared SDK change; later changes only affect
the browser/worker and their fixtures. All these are automated guest checks.

Next work remains the actual web platform: a persistent realm, DOM tree/query/event
APIs, navigation/timers/network/origin/session behavior and bounded transport of
larger executable documents. YouTube additionally needs the browser media/codecs/
MSE/A/V pipeline. These are the existing roadmap gates, not implied by this CSS
subset or by the native WAV player.

## Reproduce

```sh
# Development-only Chromium capture. It is never part of the guest application.
npm install --prefix /tmp/jbrowse-reference playwright-core@1.65.0
RUSTOS_PLAYWRIGHT_CORE=/tmp/jbrowse-reference/node_modules/playwright-core \
 RUSTOS_CHROMIUM=/path/to/chromium node tools/browser_reference.cjs \
 crates/browser/tests/fixtures/css-layout.html target/reference

./tools/build-grub-disk.sh target/jbrowse-web.img
RUSTOS_TEST_DISK=target/jbrowse-web.img RUSTOS_CPUS=8 \
 RUSTOS_JBROWSE_CSS=1 python3 e2e/jbrowse.py
RUSTOS_TEST_DISK=target/jbrowse-web.img RUSTOS_CPUS=8 \
 RUSTOS_JBROWSE_LARGE=1 python3 e2e/jbrowse.py
RUSTOS_TEST_DISK=target/jbrowse-web.img RUSTOS_CPUS=8 python3 e2e/browser_p1.py
RUSTOS_TEST_DISK=target/jbrowse-web.img RUSTOS_CPUS=8 python3 e2e/browser_perf.py
```

Host Cargo tests run from `/tmp` with the pinned nightly and explicit
`--target x86_64-unknown-linux-gnu`, as in the testing guide. The complete P8 corpus
still requires three full scheduled runs and every mandatory task; easier pages
cannot average away the open Google/YouTube/Facebook gates.
