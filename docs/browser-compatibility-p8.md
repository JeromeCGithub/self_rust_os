# P8 compatibility corpus and displayed-page probes

P8 is in progress. The [versioned corpus](browser-corpus-v1.json) fixes30 tasks:
27 ordinary tasks and three independent mandatory targets. It requires25/27
ordinary tasks and all Google search, YouTube playback and Facebook session checks
on three complete scheduled runs. Blocked and untested tasks stay in the denominator.
Changing a URL/check/profile invalidates the report's canonical manifest digest.
Reports bind evidence to a SHA256 of the tested disk image. The assessor checks
reporting discipline and evidence files; it cannot authenticate an invented report.

This page records the initial probe baseline. The subsequent
[CSS/reference and transfer work](browser-css-comparison.md) has its own tested
build/evidence and assessment; it does not turn the initial failures into passes.

The [initial partial assessment](browser-p8/assessment.json) **fails**:1/27
ordinary tasks, all three mandatory gates open, one partial run rather than three
scheduled runs. [Observations](browser-p8/report.json) retain every unrun task.
A homepage load or HTTP200 never establishes a complete search/media/session task.

## Native observation on2026-11-06

All three public probes used the same rebuilt1GiB/8CPU TCG image and1920×1080
display with640×400 browser content, including its chrome. The public load budget
is fixed at5seconds, excluding command typing/boot, including native launch,
network/scripts/layout and displayed-body readback. It was not raised after failure.

| Page | Observed result |
| --- | --- |
| CERN opening document |3.394s; HTTP200,25 distinct laid-out links, zero script errors,25898 nonbackground body pixels; submitted/displayed digest match |
| Google query |7.003s; HTTP200, three script errors, zero links and unsupported-script fallback; fails compatibility and timing |
| YouTube watch page |2.882s to document-size failure; no playback, title or committed document; fails compatibility |

The small CERN task explicitly requires only its title, readable opening body and
laid-out links. It is the only completed corpus task. The generic probe always
sets `complete_task_verified:false`; mapping it to this narrowly defined reading
task does not imply navigation or another site's interactive task.

Actual images: [CERN](browser-p8/probes/cern/page.png),
[Google failure](browser-p8/probes/google/page.png),
[YouTube loading placeholder](browser-p8/probes/youtube/page.png).
Each directory contains its native serial and JSON assertion record. The YouTube
body remains the initial placeholder on failure; its pixels are not loaded video
or proof that error chrome has reached the display.

## Probe design and red/green evidence

All diagnostics stay in Ring3. `Page::summary` exposes counts, not text/URLs.
`jbrowse URL --diagnostics` emits numeric counts and a rendered RGB fingerprint.
The SDK's safe `Canvas::region_digest` validates its complete viewport, excludes
alpha/padding and mutates no pixels. It is a noncryptographic readback comparison,
not a security primitive or presentation acknowledgment. The normal browser skips it.

The native probe waits until QEMU's opaque body interior matches the submitted
surface exactly. It excludes the compositor's one-pixel side rims. Publication
alone precedes composition: an initial counter-plus-delay probe falsely accepted
Loading pixels. [Invalidated observation](browser-p8/invalidated-placeholder.json)
and [rim failure](browser-p8/rim-observation.log) remain visible. They are not
accepted evidence. The corrected CERN readback has25898 ink pixels versus1771
in that invalidated placeholder. Error outcomes always fail, even if their initial
body fingerprint matches. QMP is automated native input, not a manual host GUI test.

Red tests observed missing [summary API](browser-p8/summary-api-red.log),
[readback API](browser-p8/readback-api-red.log),
[corpus assessor](browser-p8/corpus-red.log),
[host digest](browser-p8/readback-host-red.log), and
[native counters on the P7 image](browser-p8/probe-native-red.log).
Green: [28 UI tests](browser-p8/ui-green.log),
[28 browser tests with JavaScript](browser-p8/browser-host-green.log),
[54 tooling tests](browser-p8/tooling-green.log),
[full native browser](browser-p8/browser-native.log),
[desktop](browser-p8/desktop.log). All dependent apps/metadata and a fresh release
GRUB image were rebuilt. No kernel/ABI change was needed for P8; kernel/audio/range
coverage in P6/P7 is inherited, not rerun here.

The combined build also passes all three isolated [performance trials](browser-p8/performance.json)
with the original1s window/2s local page/1s navigation budgets: maxima0.970/1.441/0.827s.
No competing VM was run during that measurement. This local workload does not
bound public-site networking or establish a compatibility pass.

## Reproduction and open work

```sh
RUSTOS_TEST_DISK=target/jbrowse-p8.img RUSTOS_CPUS=8 \
 RUSTOS_JBROWSE_SITE=http://info.cern.ch/hypertext/WWW/TheProject.html \
 RUSTOS_JBROWSE_SITE_TITLE='The World Wide Web project' \
 RUSTOS_JBROWSE_SITE_MIN_LINKS=20 python3 e2e/jbrowse.py
python3 tools/browser_corpus.py docs/browser-corpus-v1.json docs/browser-p8/report.json
```

The second command intentionally exits1 for the incomplete public corpus. Host
assessor unit tests use synthetic reports to exercise passing/failing logic;
they do not assert that real sites passed. Most ordinary tasks, Firefox reference
layout parity, manual GUI/listening, Google results, YouTube five-minute playback
and authorized Facebook login/session/logout remain open. See the
[roadmap](browser-support-plan.md), [P6](browser-media-p6.md) and
[P7](browser-youtube-p7.md) for their prerequisites. Native image builders still
emit the known bootloader unused rustc-abi warning. These phases remain drafts.
