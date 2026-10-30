# HTTP/curl validation evidence

This records the original plain-HTTP milestone; [TLS validation](tls-validation.md)
records subsequent HTTPS support and stack expansion. This work added a Ring 3 HTTP library, the Portal curl command, a standalone CURL
application, and DNS resolution using the existing socket ABI. The driver-tree
refactor was already present in the worktree and is preserved. Toolchain remains
`nightly-2025-01-30`; no external dependency or target update was needed.

## Red evidence

- `/tmp/rustos-http-red.log`: seven of eight initial HTTP client tests failed
  against the protocol scaffold, including a fragmented Content-Length response,
  chunked response, close-delimited response, redirects, timeout and cleanup.
- `/tmp/rustos-http-e2e-red.log`: the old guest accepted the typed curl command
  and printed `unknown command`; it never produced the fixture body.
- `/tmp/rustos-http-red-launchers.log`: after CURL installation, discovery still
  reported six applications and hid ZNETTEST. Launcher capacity is now separate
  from the six simultaneous windows, and the test requires seven applications.
- `/tmp/rustos-http-stack-red.log`: the initial client exhausted Portal's 16 KiB
  user stack and double-faulted. Large parser phases were separated and packet /
  redirect record copies removed. The kernel's user-stack bounds stayed fixed.

## Passing checks executed for this change

- Pure host HTTP tests: two DNS parser cases and twelve client cases. Coverage
  includes transaction/question matching, compressed CNAME/A replies, pointer
  cycles and truncated DNS records; one-byte HTTP fragmentation, short writes,
  informational statuses, 204/304, chunk extensions/trailers, EOF framing,
  malformed/ambiguous lengths, oversized headers/body, timeout, callback failure,
  redirects and cleanup. Log: `/tmp/rustos-http-green.log`.
- All applications regenerated with `tools/build-user.sh`; final GRUB disk built
  with `tools/build-grub-disk.sh`. `.bin` and metadata are generated together.
- `e2e/http.py`: Portal curl and standalone CURL receive an independently served
  chunked body. A real Ring 3 library probe performs three rounds of DNS-backed
  GETs for Content-Length, redirect/chunked, EOF/404, truncation and 20,000-byte
  streaming. Global socket count returns to baseline after every operation.
- `e2e/http.py --site http://example.com/`: guest DNS plus an external request
  returned HTTP 200 and 577 decoded bytes. Fixture success and public success
  are separate assertions. Final logs use `/tmp/rustos-http-final-e2e.log` and
  `target/http/serial.log`; raw request fixtures are independent host sockets.
- Existing Internet ping, no-NIC and Ring 3 socket boundary/lifecycle/UDP/TCP
  regression modes pass. Logs use `/tmp/rustos-http-final-{ping,no-nic,sockets}.log`.
- Desktop E2E covers commands, file operations, simultaneous Snake, focus,
  drag/close and application launches at the updated alphabetical positions.
  Desktop performance checks cover idle CPU, keyboard bursts and input under
  CPU load. Logs use `/tmp/rustos-http-final-{desktop,perf}.log`.
- Default, GRUB debug and GRUB release builds, `cargo test --no-run`, all 61
  kernel unit cases and the complete basic_boot, gfx, userspace, display_mapping,
  should_panic, stack_overflow and keyboard_timer integration executables passed.
  Logs use `/tmp/rustos-http-kernel-*.log`.
- Documentation checking, all eleven host tooling cases and `git diff --check`
  pass. Formatting/API comments changed application hashes, so final runtime
  checks use a fresh disk built from the regenerated final binaries.

[HTTP screenshot](screenshots/http-curl.png) is captured from the tested guest.
It shows seven launchers, the decoded fixture body and status, explicit HTTPS
rejection, and subsequent prompt recovery. It supplements protocol and runtime
assertions; it does not establish host GUI behavior.

## Limits and remaining checks

[The current HTTP contract](http.md) records HTTP/HTTPS GET/IPv4 scope, DNS and
response limits, deadlines and unsupported encodings. Portal requests are synchronous
within that application while socket sleeps permit other processes to run.
Manual host display/input interaction has not been checked. Physical hardware,
IPv6 remains unsupported. HTTPS is now implemented and verified separately.

The existing debug heap stress timeout and process frame/stack reclamation gaps
remain as recorded in [networking validation](networking-validation.md); they
were not changed or claimed repaired by HTTP socket cleanup tests. The legacy
bootloader's unused rustc-abi target-field warning remains visible in kernel test
builds. No project warning was suppressed.
