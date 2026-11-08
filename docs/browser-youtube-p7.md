# P7: validated media byte ranges and the YouTube gate

P7 is **in progress**. The reusable Ring3 HTTP client now supports bounded single
byte-range GETs, a prerequisite for network-media seeking. There is still no
verified YouTube playback. PCM/WAV output is [P6](browser-media-p6.md); it does not
supply YouTube codecs, browser media elements, MSE or script compatibility.

## Range request contract

`http::Request::GetRange(ByteRange { first, last })` uses closed inclusive byte
offsets. Inverted/overflowing ranges or ranges larger than Options::max_body fail
before DNS/socket allocation. The client emits one computed `Range: bytes=x-y`
header with identity encoding; arbitrary header injection is not exposed.

A successful range requires206 and a syntactically valid single known-length
`Content-Range`. The first byte must equal the requested first byte; the last
must equal the requested last byte or the resource's final byte when shorter.
Content-Length must match that interval when present. Chunked/EOF bodies are
bounded to the same expected length and must end at exactly that length. A wrong
interval is rejected before a body callback; truncation detected later can leave
already delivered bytes, as with existing streaming errors. The caller must drop
partial output on failure.

`Response::range` exposes typed Satisfied/Unsatisfied metadata. Unknown totals,
multipart ranges, duplicate/malformed fields and arithmetic overflow are rejected
by this initial subset. For a requested range,200 returns RangeIgnored and416
returns RangeUnsatisfiable; a full file is never mistaken for a seek fragment.
Ordinary GET/form POST APIs preserve their existing signatures and behavior.
Range headers survive GET redirects; HTTPS downgrade rejection, certificate
verification, cancellation, deadlines and close-on-error remain in effect.

No ETag/If-Range/revalidation or mutable-representation assembly policy is supplied.
A client must not infer coherent multi-request playback merely from equal lengths.
The kernel/syscall ABI, dependencies and toolchain are unchanged; new policy is in
`crates/http` and the userspace adapter. Actual Ring3 consumers in ZNETTEST verify
metadata/body bytes over both HTTP and verified HTTPS.

## Evidence and remaining public task

- [Missing API red](browser-p7/range-api-red.log) records the absent request/metadata.
- The previous native image failed independent assertions requiring actual range
  requests over [HTTP](browser-p7/http-native-red.log) and
  [HTTPS](browser-p7/tls-native-red.log), after its old request suites completed.
- [Host green](browser-p7/http-host-green.log):2 unit and30 integration cases,
  including short writes, EOF-shortened intervals, ignored/unsatisfiable/wrong/
  duplicate ranges, chunk truncation, redirects and cleanup, alongside POST/GET.
- [Native HTTP](browser-p7/http-native-green.log) asserts three requests and exact
  `efgh`/bytes4–7/total12 metadata. [Native TLS](browser-p7/tls-native-green.log)
  asserts two verified ranges, two POSTs and preserved certificate/error/cleanup
  coverage. Their legacy PASS summaries occur after these new host assertions.
- [Desktop](browser-p7/desktop.log), [tooling](browser-p7/tooling.log), all app/
  metadata rebuilds, fresh release GRUB image, docs checker and diff checks pass.
  A screenshot cannot establish byte-offset/framing correctness, so this protocol
  increment supplies native request/body assertions and logs rather than an
  unrelated UI image.

Reproduce with `RUSTOS_TEST_DISK=target/jbrowse-p7.img RUSTOS_CPUS=8` and the native
`e2e/http.py`, `e2e/tls.py`, `e2e/desktop.py` runners after rebuilding that image.
Host HTTP tests use the pinned nightly and explicit Linux host target from `/tmp`.
No kernel runtime change was made; P6's kernel evidence is inherited, not rerun.

The full P7 task still requires a real public watch page, synchronized unencrypted
video and sound, pause/two seeks/volume/captions/fullscreen, five minutes of
playback and measured startup/rebuffer/drops/A/V drift. No host renderer, saved
stream in another player or thumbnail counts. Google results and Facebook account
sessions remain independent failed/untested goals. Manual host GUI and listening
checks, compressed codecs, HTML media/MSE and the complete public task remain open.

Reference checked2026-11-06:
[RFC9110 range requests](https://www.rfc-editor.org/rfc/rfc9110.html#name-range-requests).

The rebuilt [full native browser regression](browser-p7/browser.log) also passes:
script workers, password/POST/303, fonts/PNG, real CERN, resize/restore, ordinary
GET forms/navigation/error recovery and close/relaunch. This is regression coverage,
not YouTube acceptance.
