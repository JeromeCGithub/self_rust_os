# Terminal/shell separation validation

The initial implementation was validated on `feat/terminal-forge-stdio`, based
on main `d9a9613`. The networking-main rebase is recorded below.
Portal now starts an independent Forge process and renders its output; command
parsing and execution remain in Forge. See [architecture and limits](terminal-shell.md).

## Red evidence

- `python3 e2e/desktop.py` against the original disk failed with
  `missing [portal] shell pid=`: Portal did not spawn a shell.
- `cargo test --test stdio` with the connection stub failed at
  `spawned shell must connect to host`.
- The screen model's two host tests failed before text insertion/wrapping/editing
  were implemented; both pass with the completed model.
- The owner-death regression failed with `Some(0)` rather than the scheduler's
  block sentinel when a hosted child killed its host. Kill now schedules away
  from a child stopped as part of host teardown.
- The first performance run measured 26% CPU for Snake with a terminal, above the
  unchanged 25% budget. Caching the immutable SDK I/O role removed repeated
  metadata syscalls and restored the existing performance checks.

An initial attempt to extend the large framebuffer fixture exhausted its available
frames, before testing the new connection behavior. The connection checks moved
to an independent 640×480 RAM fixture; the original large framebuffer test remains
unchanged. Existing frame/stack leaks are not fixed by this change.

## Checks executed

- All seven applications and their binary/metadata pairs rebuilt through
  `CARGO_NET_OFFLINE=true ./tools/build-grub-disk.sh`, including the release GRUB kernel.
- `cargo build`, `cargo build --features grub`, `cargo test --no-run` passed.
- `cargo test --lib --test stdio --test basic_boot --test gfx --test display_mapping --test keyboard_timer --test userspace --test should_panic --test stack_overflow`
  passed. The new stdio fixture verifies private endpoints, metadata, FIFO order,
  full-queue backpressure, unavailable peers, endpoint reuse and owned-child
  stop/reaping, including killing the host from its child.
- `cargo test --release --test heap_allocation` passed all four cases.
- `python3 e2e/desktop.py` passed on the final rebuilt disk: independent Forge
  spawning, filesystem results, shell-only commands through Portal, error output,
  normal shell exit, hosted-child reaping after terminal close, focus/drag/close,
  concurrent Snake and compositor survival after DE exit.
- Native Rust harnesses passed: screen 2, UI 18, glass 6 and damage 4 cases.
- Documentation checker, all seven checker regression cases and whitespace checks passed.

On the final rebuilt disk, `python3 e2e/glass_ui.py` and
`python3 e2e/desktop_perf.py` also passed. Glass checks include the Ring 3 invalid
stdio probes, button capture, focus and live lower-layer damage. Performance
retained the existing thresholds: keyboard burst drain 51 ms, Snake with terminal
22% of one host core, and pointer latency under CPU load 41 ms.

## Evidence and limits

[Terminal screenshot](screenshots/desktop-terminal.png) shows command output
received from Forge inside Portal's glass window.
[Multitasking screenshot](screenshots/desktop-multitasking.png) shows the terminal
alongside Snake. Pixel and input assertions supplement the screenshots; actual
process spawning and child reaping are asserted from serial diagnostics.

QMP input was tested; manual host GUI input remains unverified. The unchanged
debug heap stress timeout was not rerun or repaired. The bootloader's existing
unused `rustc-abi` target-field warning remains. This change does not claim frame
or kernel-stack reclamation, general process-control permissions, a PTY, ANSI
support, job control, or inherited stdio for further child programs. Startup spawn
errors are covered through the SDK's failed-path probe; manually booting Portal
with its Forge executable removed was not tested.

## Rebase onto networking main (67d7fed)

The branch was rebased onto main `67d7fed` (PR #36) without replacing its driver,
network, HTTP/TLS, platform or memory-layout implementations. Source comparison
confirmed those subsystems and their shared crates are retained. Their generated
application artifacts were rebuilt with the connected-I/O SDK.

Main's Portal `ping` adapter and HTTP/HTTPS command policy now live in Forge.
Forge retains the 512-byte command buffer, asynchronous ping and TLS feature/
software-crypto flags. Portal remains a presentation/input process. The standalone
CURL app, syscall ranges 64–70 and 80–81, 1 GiB QEMU profile, expanded stacks/heaps
and actual-file-size loader remain available.

Additional red/green evidence from this rebase:

- The first rebased disk reported `unknown command: ping`. Moving networking
  policy into Forge restored packet-verified ping without a Portal parser.
- Main's automatic blocked-wait reaping initially left a stale host endpoint:
  `stdio(0)` returned 12 instead of zero. A new connected-waiter regression
  verifies both the exit register and released endpoint.
- A foreign waiter could auto-reap the calling child during owner teardown.
  The regression observed `Some(0)` instead of the block sentinel. Kill now
  schedules away from a caller that has either stopped or already been reaped.
- The old sixteen-packet transport made GitHub HTTPS and a new deterministic
  20,000-byte local HTTP response time out. Queues now reserve 4096 packets
  (16 KiB per endpoint) fallibly during connection setup, with no allocation
  during delivery. Portal drains a matching bounded batch and paints once.
  Full-queue rejection, FIFO and endpoint-reuse tests cover the new bound.
  Existing HTTP deadlines and all performance thresholds remain unchanged.

Checks run on the integrated build:

- Rebuilt all nine applications and metadata through
  `CARGO_NET_OFFLINE=true ./tools/build-grub-disk.sh`.
- Default/GRUB builds and `cargo test --no-run` passed.
- `cargo test --lib --test stdio --test basic_boot --test gfx --test display_mapping --test keyboard_timer --test userspace --test should_panic --test stack_overflow`
  and release heap allocation tests passed.
- Host net ABI, platform ABI, stack, ping, HTTP and TLS crate tests passed;
  screen/UI/material/damage harnesses passed 2/18/6/4 cases.
- Desktop and glass E2E passed with independent Forge processes, command/error
  results, close/exit cleanup, pointer/focus and backdrop behavior.
- HTTP E2E passed, including the new 20,000-byte hosted-shell response and
  standalone CURL/SDK probes. TLS E2E passed verified RSA/hostname/date/trust/
  redirect/streaming cases and `--site https://github.com/`: HTTP 200,
  **530,080 body bytes**. Separate `--no-rng` and `--bad-clock` runs passed.
- Ping to 10.0.2.2 and 8.8.8.8 each produced two packet-verified replies.
  Missing NIC behavior and the Ring 3 network lifecycle/UDP/TCP probe passed.
- Desktop performance passed unchanged budgets: idle terminal 7%, terminal plus
  Snake 23%, keyboard burst 71 ms, loaded pointer 53 ms.
  `e2e/net_perf.py --label rebased --assert-fast --site http://google.com/`
  passed: two 128 KiB rounds in 3/2 PIT ticks; public HTTP 200, 87,471 bytes,
  approximately 1.96 seconds.
- Documentation checking, all 24 host tooling cases and whitespace checks passed.

The first network lifecycle probe run, concurrent with other QEMU suites, hit its
bounded receive deadline. Its unchanged assertions passed when rerun in isolation;
no timeout or expectation was weakened. The original failure remains recorded
separately from the passing rerun.

Fresh [desktop evidence](screenshots/desktop-terminal.png) and
[GitHub HTTPS evidence](screenshots/github-curl.png) were captured and inspected
from the integrated build. QMP input remains distinct from an unperformed manual
host GUI check. Existing debug heap timeout, frame/stack reclamation and general
process-permission limitations remain as documented above.
