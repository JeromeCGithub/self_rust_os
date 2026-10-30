# Terminal and shell processes

Portal is a terminal window. Forge is a separate shell executable. Opening
Terminal starts `/BIN/PORTAL.BIN`; Portal creates its glass window and launches
`/BIN/FORGE.BIN` with `userlib::process::Child::spawn`.

| Component | Responsibility |
| --- | --- |
| Portal event loop | Window events, keyboard forwarding, output draining and repaint scheduling |
| Portal screen model | Bounded text rows, wrapping, scrolling, carriage return and backspace |
| Forge | Prompt, line editing, command parsing, filesystem commands and executable search/launch policy |
| User SDK | Standard-I/O packet framing, retry/backpressure and owned child cleanup |
| Kernel | Endpoint ownership, bounded FIFO delivery, process creation and endpoint lifetime |
| Compositor | Window placement, stacking, focus, decorations and composition |

The terminal contains no command parser. Forge works both as a standalone console
application and as a hosted shell. The SDK redirects `sys::read`,
`sys::read_nb` and `sys::console_write` only on the child side of an explicitly
connected spawn. The host retains physical keyboard focus and forwards input;
the hosted shell never consumes another window's hardware keyboard queue.
Compositor messages use their existing independent inbox.

## Application interface

```rust
use userlib::process::Child;

fn main() {
    let mut shell = Child::spawn("/BIN/FORGE.BIN").expect("shell executable");
    // In an event loop, drain shell.read() before forwarding keyboard bytes.
    // shell.write_byte(byte) returns Full when the same byte must be retried.
    // Dropping shell kills and reaps the child.
}
```

Each process supports one connected endpoint. A connected child cannot own a
second connection. Ordinary `sys::spawn` keeps its original console behavior;
streams are not inherited by further children. The kernel does not inspect
executable names or interpret bytes as shell commands.

The transport has 4096 scalar packets per direction. Input packets contain one
byte; output packets contain up to three bytes. A full queue never overwrites
earlier packets. Portal drains output before sending more input and retains the
next unsent byte. Child-side blocking reads and full output queues sleep for one
PIT tick before retrying. Normal console applications keep the original syscalls.

After a child exits, the host drains buffered output and then observes EOF.
Portal reaps the shell and exits, allowing the compositor to remove the window.
Closing Portal explicitly drops the child. If the terminal is killed or exits
without dropping it, the kernel stops its owned child and reaps that child's PCB
when the terminal is reaped. Reaping the child independently releases the host's
endpoint so it can spawn another child. This does not repair existing address-space
frame or kernel-stack reclamation gaps.

## Current limits

This is a small duplex standard-I/O connection, not a POSIX PTY or file-descriptor
implementation. There is no ANSI escape parser, job control, signal delivery,
terminal-size negotiation, stderr separation or general pipeline support.
The screen supports printable ASCII, newlines, carriage return and backspace.
Forge preserves main's 512-byte input limit, asynchronous ping and HTTP/HTTPS curl commands. Standalone Forge retains its existing
console presentation. Commands launched by Forge use ordinary spawning and do
not inherit its terminal connection.

See the [syscall contract](syscalls.md#connected-child-standard-io) for wire
values and [testing](testing.md) for the existing execution matrix.

See [validation evidence](terminal-validation.md) for red/green checks and remaining gaps.

Networking commands and the TLS-enabled heap live in Forge; Portal has no
HTTP/TLS dependencies. The independent CURL application remains available.

Endpoint queues reserve 16 KiB each only on connected spawning. Allocation
failure leaves both endpoints unchanged; reaping releases their queue storage.
The larger bounded output batch preserves HTTP streaming within existing deadlines.
