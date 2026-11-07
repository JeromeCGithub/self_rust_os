# CSS, document and transfer evidence

See the [implementation/report](../browser-css-comparison.md). These are actual
host/QEMU results, not a claim that the complete public-site corpus passes.

- Layout red: `layout-api-red.log`, `layout-geometry-red.log` (all five initial
  reference bounds wrong), `layout-boundary-red.log`, `input-title-red.log`,
  `radius-red.log`, `flex-overflow-red.log`, `link-hit-red.log`.
- Tool red/green: `reference-tool-*.log`, `readback-tool-*.log`. The native rim
  observation and its new host regression are `native-css-rim-observation.log`
  and `native-rim-host-red.log`. Only the declared compositor rims are excluded;
  other shifts still fail. `chromium-fixture.json/png` are the actual browser oracle.
- Document red/green: `large-document-*.log`, `script-classification-*.log`,
  `large-native-*.log`, and `large-document/` with original screenshot/serial/JSON.
- Packet/worker red/green: `packet-api-red.log`, `packet-green.log`,
  `worker-protocol-*.log`, `worker-native-red.log`, `worker-native-8cpu.log`.
  The parent-baseline allocation observation remains in its own log. The exact
  frame-recovery checks were preserved. Final CPU-profile logs/serial are added
  alongside them after their runs complete.
- Host green: `browser-host-green.log` (43), `dom-host-green.log` (7),
  `worker-protocol-green.log` (5), `packet-green.log` (1), `tooling-green.log` (57).
- Performance: `performance.json/log`, measured alone on the declared8CPU profile.
  All three fresh processes pass the original1s/2s/1s budgets. The large-page
  timing was also measured alone; other native regression runs are not timing claims.
- Google references: `firefox-live-google.png` is a fresh normal Firefox request;
  `firefox-same-markup.png` and `chromium-same-markup.png` render the captured
  RustOS-user-agent response. That response is different from normal Firefox's
  served modern markup. The before native image is `jbrowse-home-before.png`.
  The captured HTML/headers/cookies are not committed.

Native images and mutable QEMU disks remain under `target/`, outside Git. Tests
copy images privately; no image is rebuilt while attached to a VM. Automatic QMP
input and headless browser captures do not establish manual host-desktop input or
host-speaker audio. Public search/video/session tasks remain open.

Final evidence uses image SHA256
`b2e547d791121274f671ba0d009079c7b14746e4ff1b834512c4eebfee4f4e72`.
`latest/*/body.png` are exact body crops, not synthetic renders; JSON and serial
identify each outcome. The final native CSS test also measures the proportional
field glyphs. `jbrowse-home-after.png` retains the full desktop frame.

`browser-native-green.log`, `worker-native-{1,8,32}cpu.log` and their serial logs
record final-image integration. `desktop-green.log` is the earlier shared-SDK
regression; browser-only follow-ups do not change the desktop binary.
`sheets-native-red.log` and `sheet-budget-red.log` reproduce the old four-sheet
limit. `stylesheet-report-red.log` reproduces the missing failure notification;
`style-failure-green.log` proves fallback and subsequent navigation recovery.
`before-sheet-extension/` retains the earlier public outcomes with their own build
digest; they are not relabeled as final-image passes.

The final corpus assessor deliberately returns1; Google/YouTube/Facebook are not
working task passes. Screenshots show actual failures as well as successes.

Readable worker excerpts are `worker-{1,8,32}cpu-summary.txt`; the corresponding
raw serial logs remain alongside them. `build.json` binds the image, binaries,
pinned toolchain and final performance record. The native CSS check also rejects
the old bitmap field text: its measured dark glyph bounds are63×13 pixels for
the18px reference input, allowing the installed outline-face substitution.

Copied command transcripts normalize trailing horizontal whitespace and final
blank lines for diff hygiene. Raw serial files, JSON measurements and image pixels
are preserved. Original command outputs remain in the ignored local test artifacts.
