# TLS validation evidence

This records the initial TLS milestone. Subsequent [HTTP/memory limit validation](http-limits-validation.md)
records the larger heap/stack, GitHub fix and new test results.

Validated on 2026-10-30 in branch `plan/qemu-ethernet-ping`, worktree
`/home/jay/personal/os/space1.2.3.4....`. This adds verified userspace TLS/HTTPS
on top of the earlier HTTP and network work; the pinned nightly remains
`nightly-2025-01-30`. Results below were executed during this TLS implementation,
not inferred from compilation or inherited screenshots.

## Red evidence

Temporary logs remain local to the development host, not repository artifacts.

| Regression | Observed failure before implementation |
| --- | --- |
| `/tmp/rustos-https-url-red.log` | HTTPS URL/client tests rejected the unsupported scheme |
| `/tmp/rustos-tls-e2e-red.log` | Real Portal HTTPS request could not perform TLS |
| `/tmp/rustos-tls-rng-red.log`, `/tmp/rustos-tls-rtc-red.log` | New entropy/calendar tests lacked the requested driver behavior |
| `/tmp/rustos-tls-platform-red.log` | QEMU profile lacked explicit hardware entropy/UTC configuration |
| `/tmp/rustos-tls-stack-red.log` | Complete 128 KiB stack mapping assertion failed on the old 16 KiB mapping |
| `/tmp/rustos-tls-stack-guest-red.log` | Rustls initialization exhausted the old user stack and double-faulted |
| `/tmp/rustos-tls-deadline-red.log` | Buffered plaintext was returned after deadline expiry; now returns Timeout |
| `/tmp/rustos-tls-aes-red.log` | TLS handshake failed when the server offered only AES-GCM |

Test-fixture issues found and corrected separately: the TLS mode file needed an
8.3-compatible extension (`TLS.MOD`); the public request needed to focus an exposed
part of Portal; a socket probe's old invalid address became mapped after stack
expansion, so it now crosses the actual guard with the same rejection assertion.
Host driver tests initially used the ordinary Rust test attribute in the custom
kernel harness; conditional attributes now execute all seven in both harnesses.
A host Cargo invocation from the bare-metal root failed by mixing rebuilt core
with host std; host commands below run from `/tmp` as documented.

## Passing checks

Host checks, using exact nightly and explicit `x86_64-unknown-linux-gnu` target
from outside the root Cargo configuration:

- `cargo test --manifest-path "$REPO/crates/tls/Cargo.toml" ...`: six cases,
  covering fragmented verified ECDSA and RSA handshakes/data, each of three TLS 1.3
  cipher suites, untrusted/wrong-name/expired/future certificates, bad clock,
  unexpected EOF, expired deadlines with buffered data, and authenticated-record corruption.
- HTTP: two DNS and fifteen client cases, including TLS upgrade-before-GET,
  certificate errors, HTTPS redirects and downgrade rejection.
- Platform ABI: one error-encoding case. Net ABI: two; net-stack: five; ping: three.
  Net-stack's allocation-failure example also passed.
- Standalone `rustc --test` driver/page checks: RDRAND three, CMOS four,
  effective page permissions two, PCI decoding two.
- `python3 -B -m unittest discover -s tools/tests -p 'test_*.py'`: seventeen cases.

Fresh `tools/build-grub-disk.sh` runs rebuild every app with
`tools/build-user.sh`, regenerate binaries/metadata, and build the release GRUB
kernel. Runtime checks use actual guest processes and syscalls:

| Command | Verified behavior |
| --- | --- |
| `python3 -B e2e/tls.py --site https://google.com/` | Independent OpenSSL fixtures plus public HTTPS with default Mozilla roots, HTTP 200; screenshot linked below |
| `python3 -B e2e/tls.py --no-rng` | Missing RDRAND returns typed failure; no HTTP request reaches the TLS fixture; prompt recovers |
| `python3 -B e2e/tls.py --bad-clock` | Invalid UTC refuses HTTPS, no HTTP request sent; prompt recovers |
| `python3 -B e2e/http.py` | Existing Portal/CURL/plain HTTP, DNS and cleanup regressions |
| `python3 -B e2e/network.py --address 8.8.8.8` | Four packet-verified ICMP replies and recovered shell |
| `python3 -B e2e/network.py --no-nic` | Missing NIC reported without breaking shell |
| `python3 -B e2e/network.py --probe` | Ring 3 buffer validation, socket ownership, UDP/TCP/HTTP and exit/kill cleanup |
| `python3 -B e2e/net_perf.py --label tls-aes --assert-fast` | 128 KiB in 3 and 2 PIT ticks; advertised TCP receive window 32 KiB |
| `python3 -B e2e/desktop.py` | Pixel/input/focus, simultaneous applications, shell operations, drag/close regressions |
| `python3 -B e2e/desktop_perf.py` | Idle/terminal/Snake CPU budgets, keyboard bursts and input under load |

The TLS guest probe explicitly checks hostname, trust and date failures before
any application callback; RSA and ECDSA leaf certificates; chunked and 64 KiB
streaming; downgrade refusal; two rounds returning socket count and allocator use
to baseline. A child opens a socket and deliberately exhausts its heap: it exits
with code 12, parent wait/reap succeeds, and the socket count returns to baseline.
Raw platform calls test null/supervisor/overflow/cross-guard buffers, oversized
requests, reserved arguments and zero-length output. Fixture certificates are
regenerated into a disposable directory each run; their CA is added explicitly
only by the probe. Default Portal rejects that CA.

Kernel checks executed:

- `cargo build`, debug/release `cargo build --features grub`, `cargo test --no-run`.
- `cargo test --lib`: 69 QEMU runtime cases.
- Complete selected integrations: basic_boot, gfx, userspace, display_mapping,
  should_panic, stack_overflow, keyboard_timer. Display mapping checks all 128 KiB,
  unmapped stack guards, zeroed independent stack endpoints, and repeated spawning.
- `cargo test --release --test heap_allocation`: four cases.
- `bash e2e/ata.sh`, `bash e2e/fat32.sh`: both two-case disposable-fixture suites.

Linked release ELFs for Portal, CURL and ZNETTEST were disassembled and inspected
for SIMD/MMX/x87 register instructions: none found. Crypto dependencies explicitly
select scalar backends; this check must be repeated after dependency updates.
Documentation checks, shell syntax and final `git diff --check` are also required.

[HTTPS screenshot](screenshots/https-curl.png) is captured from the tested public
Google request in Portal, with its HTTP 200 summary. Serial/QMP evidence is in
`target/tls-verified/`; failure profiles use `target/tls-no-rng/` and
`target/tls-bad-clock/`. Logs and images in `target/` are intentionally untracked.
The response body/size can change between public requests; public connectivity
and CA validity remain external dependencies.

## Remaining failures and limits

`cargo test --test heap_allocation` still times out at the unchanged 60-second
limit in the 33,554,432-allocation debug stress case. This documented baseline was
reproduced again during TLS verification; the release suite passes. It was not
weakened, skipped or repaired by TLS work. Bootloader dependency builds still
report the known unused `rustc-abi` JSON-field warning.

The RustCrypto provider is experimental and upstream warns against production
use; [TLS architecture](tls.md) records the exact protocol, trust and memory limits.
The host-controlled RTC/RDRAND are trusted; no authenticated time, entropy health
tests or online revocation fetching are implemented. No security audit is claimed.
Repeated TLS allocation/socket cleanup does not repair existing process page-table,
frame and kernel-stack reclamation gaps. Fatal userspace OOM terminates the app.

QMP input and screenshots establish guest behavior. Manual host GUI checks on
COSMIC/Wayland/X11 remain unperformed; no host pointer-capture claim is made.
No TLS 1.2-only site, IPv6, client authentication, HTTP/2 or arbitrary public-site
compatibility is claimed. Validation here describes the networking branch before integration with current main.
