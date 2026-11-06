# P5: password controls and form POST foundations

P5 is **in progress**. jbrowse can edit masked password controls and submit bounded
URL-encoded POST forms over HTTP or verified HTTPS. This increment does not provide
cookies, persistent sessions or a working authenticated Facebook interface.

## Architecture and behavior

All new behavior belongs to Ring3. The kernel's existing socket, TLS time/entropy,
thread and process mechanisms are unchanged; there are no syscall/target/toolchain
or dependency changes.

- The browser library parses `input type=password` as an editable control with the
  same geometry as a text field. Its display API returns one ASCII asterisk per
  Unicode scalar rather than the value. Field diagnostics omit values entirely.
- `Form::submission` constructs an owned `Submission::Get(url)` or
  `Submission::Post { url, body }`. GET keeps the previous query-replacement behavior;
  POST preserves the action query/fragment and carries successful controls in a
  separate UTF-8 URL-encoded body. Hidden fields and the activated named submitter
  are included; disabled/unnamed fields and other submit buttons are excluded.
- Encoding is restricted to `application/x-www-form-urlencoded`. Unsupported
  methods, encodings and active/local action URLs fail before submission. Limits:
  existing16 forms/64 fields,2048-byte names/values,2048-byte resolved URLs and
 16KiB encoded POST bodies. Multipart/files, full form validation and line-ending
  normalization are not implemented. The legacy `Form::submit` remains GET-only.
- The controller queues the typed request on its existing cancellable network
  worker. The address/history store URLs only. Form bodies are not sent to the
  script worker or written to serial diagnostics. Once queued, displayed password
  values and the editor are cleared; this is not cryptographic heap erasure.
- Reload and Back/Forward navigate by GET. This deliberately avoids automatic
  POST replay; caching POST responses and a user-mediated resubmission prompt are
  future work. A 303 result page can be reloaded normally without resending login.

The reusable HTTP crate exposes `Request::Get` and `Request::FormPost(&[u8])` through
`request_with_url`. The SDK supplies that API and `request_cancellable`; existing
GET calls delegate without changing their public signatures. The body is borrowed
only while the synchronous call is active, with no second complete HTTP buffer.
Fixed content type and computed Content-Length prevent callers injecting headers.
Headers and body use the same bounded short-write/cancellation/deadline path.
Socket/TLS ownership closes every completed/error/redirect connection.

301/302/303 change POST to GET and drop its body/content headers. 307/308 retain
POST only when scheme, case-insensitive hostname and effective port match. Other
body-preserving origins return `CrossOriginRedirect` before opening the target;
HTTPS downgrades still return `InsecureRedirect`. This conservative replay policy
is more restrictive than a general browser; it does not implement the full Fetch
credentials/CORS/origin model. Direct cross-origin form actions are not blocked.
HTTPS always retains certificate/hostname/time verification. Plain HTTP forms
remain plaintext; the local HTTP test uses an explicitly disposable fake password.

## Remaining authenticated-app gate

No account credentials or private Facebook content were used. The independently
owned local fixture proves form mechanics only, not authentication/session support.
Cookie/domain/path/Secure/HttpOnly/SameSite policy, redirect sessions, Origin and
Referrer policy, CSRF integration, storage/clear-data, persistent document events,
async requests, broader DOM/images and viewport APIs remain open. A Facebook
login/feed/restart/logout test requires a consenting authorized account and these
missing browser capabilities. No posting, messaging or account automation is
included. See the [P5 acceptance criteria](browser-support-plan.md#p5--facebook-and-authenticated-applications).

## Validation and references

The [P5 evidence index](browser-p5/README.md) distinguishes missing-API/native red
results, host protocol tests, real input/mask pixels, independent server body
capture, verified HTTPS POST, TLS rejection/cleanup and remaining limitations.
A screenshot of a local form is not Facebook support or proof of host GUI input.

Reference targets checked2026-11-06:
[HTML form submission](https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission)
and [Fetch redirects](https://fetch.spec.whatwg.org/#http-redirect-fetch).
These references guide the declared subset; no WPT conformance claim is made.
