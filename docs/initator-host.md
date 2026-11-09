# Initator host development backend

[The Linux backend](../tools/service-host/src/lib.rs) runs real foreground processes
using the same `rustos-service-core` catalog, graph, manager, readiness, deadlines,
restart and journal logic as native Initator. It is a development/test harness,
not a replacement for native security or QEMU tests. It adds no external crates,
Tokio, custom executor or Linux compatibility layer to the OS.

Run commands from `/tmp` to avoid the repository's bare-metal Cargo target/build-std
configuration. Substitute the absolute checkout location below:

```bash
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path /home/jay/personal/self_rust_os-initator/tools/service-host/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 build --manifest-path /home/jay/personal/self_rust_os-initator/tools/service-host/Cargo.toml --target x86_64-unknown-linux-gnu --bins
```

The executable takes `DIRECTORY DURATION_MS` (1–60000). The directory uses the
[native unit grammar](initator-units.md); host `Exec` paths point to host executables,
not guest `.SVC` images. All files are validated before `boot` is activated.
For a reproducible example, create an otherwise empty temporary unit directory:

```ini
# A.UNIT: replace Exec with the absolute built service-fixture path
Version=1
Kind=service
Name=beacon
Exec=/absolute/checkout/tools/service-host/target/x86_64-unknown-linux-gnu/debug/service-fixture
Arg=notify
Ready=notify
StartTimeoutTicks=100
```

```ini
# BOOT.UNIT
Version=1
Kind=target
Name=boot
Requires=beacon
```

Run the built `initator-host /absolute/unit-directory 1000`. After its finite run,
it kills/reaps owned children **before** printing its bounded event trace. The CLI
integration test creates these files automatically and asserts actual readiness;
an invalid-catalog test requires rejection before any child launch.

## Runtime and transport

The synchronous actor calls `poll` on Linux pidfds, nonblocking Unix streams and a
private worker-result wake stream, with the nearest policy/test deadline as its
timeout. Exit readiness triggers `Child::try_wait`; there is no periodic scan of
all child processes. Kernel-reported exits are handled before queued Ready/log
messages from dead processes. A single spawn worker uses bounded queues; actor-side
pending requests retain backpressure without consuming a failed attempt. Result
publication precedes wake delivery; wake draining precedes result draining.

`CommandExt::create_pidfd(true)` and `ChildExt::pidfd` use the pinned nightly's
`linux_pidfd` feature. Failure to obtain a pidfd fails and cleans up that launch;
there is no numeric-PID fallback for published children. Termination uses
`PidFd::kill`, so an old logical service identity cannot kill a recycled Linux PID.
See the primary [pidfd manual](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
and [poll manual](https://man7.org/linux/man-pages/man2/poll.2.html).
The small FFI boundary is isolated in [poll.rs](../tools/service-host/src/poll.rs).

The worker connects child stdin (Init), stdout (notify) and stderr (log) to
independent streams. Each application payload uses a u32 little-endian byte length
followed by the same bounded service ABI message used natively. Partial reads and
writes preserve progress; oversized, zero and truncated frames fail the stream.
Budgets temporarily remove receive interest and wake on a deadline. EOF removes
interest rather than spinning on HUP. The test fixture is a protocol-speaking
program; arbitrary stdout text does not become a native log record.

Host ticks are 10 ms for fast tests; they are not guest timer ticks or a timing
conversion promise. Synthetic process keys are monotonic, unreused identifiers;
OS pidfds separately bind actual child ownership. The journal holds 256 records,
the diagnostic trace 2048 events with an explicit overwrite counter. No disk log
I/O occurs in the supervision path.

## Tests and limits

Seven runtime tests cover Ready/exit, capped failures with fresh identities,
blocking without busy polling, stale-key kill denial, a deterministically blocked
spawn worker while a sibling watchdog expires, late-publication cleanup, log flood,
malformed framing, and eight admitted launches against the bounded worker queue.
Two CLI tests cover actual catalog execution and rejection. The host runtime uses
real process exits and streams; mocks remain useful in pure core tests.

This backend runs trusted foreground fixtures as the invoking host user. It does
not sandbox host executables, authenticate descendant writes to inherited streams,
reap arbitrary grandchildren, implement native grant enforcement, provide user
sessions, or implement system shutdown. Descendant scope ownership is M5; native
credentials/resource enforcement is M5. Catalog file checks are development input
validation, not a race-proof filesystem security boundary. Native tests remain
required for user-pointer validation, explicit grant moves, peer attribution,
process/frame cleanup, and kernel wake/park behavior.
