# Terminal and shell processes

Portal is a terminal window. Forge is a separate shell executable. Opening
Terminal starts `/BIN/PORTAL.BIN`; Portal creates its glass window and launches
`/BIN/FORGE.BIN` with `userlib::process::Child::spawn`.

| Component | Responsibility |
| --- | --- |
| Portal renderer thread | Window events, screen snapshots and repaint scheduling |
| Portal I/O thread | Owns the shell connection, forwards focused input and drains output independently of compositor acknowledgements |
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
next unsent byte. Child-side blocking reads and full output queues park on kernel readiness, then
recheck the operation after a wake. Forge uses the same event mechanism for input;
only active asynchronous ping retains a bounded deadline. Normal console applications keep the original syscalls.

After a child exits, the host drains buffered output and then observes EOF.
Portal reaps the shell and exits, allowing the compositor to remove the window.
Closing Portal exits its process, stopping the renderer, I/O thread and owned child. If the terminal is killed or exits
without dropping it, the kernel stops its owned child and reaps that child's PCB
when the terminal is reaped. Reaping the child independently releases the host's
endpoint so it can spawn another child. Deferred reclamation retains frames and stacks until every CPU has stopped
executing the retired context and TLB invalidation has completed.

## Current limits

This is a small duplex standard-I/O connection, not a POSIX PTY. Regular-file
descriptors use a separate filesystem API. There is no ANSI escape parser, job control, signal delivery,
terminal-size negotiation, stderr separation or general pipeline support.
The screen supports printable ASCII, newlines, carriage return and backspace.
Forge preserves main's 512-byte input limit, asynchronous ping and HTTP/HTTPS curl commands. Standalone Forge retains its existing
console presentation. Commands launched by Forge use ordinary spawning and do
not inherit its terminal connection.

See the [syscall contract](syscalls.md#connected-child-standard-io) for wire
values and [testing](testing.md) for the existing execution matrix.

See [validation evidence](terminal-validation.md) for red/green checks and remaining gaps.

Networking commands and HTTPS libraries live in Forge; Portal has no
HTTP/TLS dependencies. The independent CURL application remains available.

Endpoint queues reserve 16 KiB each only on connected spawning. Allocation
failure leaves both endpoints unchanged; reaping releases their queue storage.
The larger bounded output batch preserves HTTP streaming within existing deadlines.

A process-local mutex protects the text model. After the previous frame release,
the renderer copies the latest text snapshot and releases the mutex before painting.
The compositor releases the submitted frame after presentation, bounding stale
frames while the I/O thread continues independently.
The I/O worker publishes a revision with release ordering and wakes the renderer's
process-private atomic-word wait. The renderer acquires the revision before reading
text; its one-tick deadline services window messages and blink animation when idle.
See [two-terminal latency evidence](parallel-latency-validation.md).

## Filesystem commands and medit

All commands below run inside the Forge shell in Ring 3. **`cd` is a shell
builtin**, so it changes Forge's working directory; Portal only forwards input
and renders output. The kernel owns per-process cwd and file descriptors, checks
paths and mappings, and performs FAT32 operations. It does not parse commands.

| Command | Behavior |
| --- | --- |
| `cd [path]` | Change Forge cwd; no argument selects `/`. Failed changes preserve cwd. Supports absolute/relative paths, `.` and `..`. |
| `pwd` | Print the actual canonical cwd (uppercase FAT32 spelling). |
| `ls [path]` | List a directory; no argument selects cwd. |
| `mkdir path` | Create relative/absolute directories with missing parents (existing `mkdir -p` behavior). |
| `cat file` | Stream bytes through an owned descriptor until EOF; no former 1 KiB file limit. Adds a final newline. |
| `write file text` | Replace a file relative to cwd or by absolute path. |
| `medit file` | Open the basic modal text editor within the shell's standard-I/O connection. |

`medit` supports ASCII text, LF newlines and stored tabs, up to 16 KiB. Opening
a missing file creates an empty file. It rejects directories, unreadable files,
non-ASCII/CRLF content and oversized files. New files remain empty on disk until
saved; quitting without saving leaves that initially created empty file.

Start in normal mode: `i` inserts before the cursor, `a` inserts after it, `h/j/k/l`
move, `0`/`$` select line start/end, and `x` deletes. Insert mode accepts text,
Enter and Backspace; Escape returns to normal mode. `:w` saves, `:q` quits only
when clean, `:wq` saves then quits, and `:q!` discards edits. Failed saves retain
the edited buffer. There is no undo, search, selection, syntax highlighting or
arrow-key protocol. Tabs display as one space and retain their stored byte.

The terminal has no ANSI parser. The editor redraws a bounded scrolling page,
shows `|` at the cursor, and displays its mode and dirty marker. This works through
Portal and standalone Forge using the same shell logic. No command/editor code
was added to Portal. See the [ABI](syscalls.md#regular-files-and-working-directories),
[regression evidence](file-commands-validation.md) and
[editor screenshot](screenshots/desktop-medit.png).

## Launching programs with arguments

Forge parses executable arguments in Ring 3; Portal only forwards bytes. For
example, `./hello alpha "two words" ""` launches `/bin/HELLO.BIN` with four argv
entries: executable path, `alpha`, `two words`, and an empty string. Single/double
quotes group text; backslash escapes the next character outside single quotes.
Unterminated quotes/escapes and more than 32 words fail without launching a child.
There is no variable expansion, globbing or command substitution. The existing
512-byte input limit remains.

The `./name` shorthand still searches `/bin/NAME.BIN`. A name containing a slash
or an extension is an explicit path resolved against cwd; absolute executable
paths are also accepted. Spawn and argument passing use the typed SDK; kernel
code does not tokenize a command or search for executables. See
[the startup ABI](syscalls.md#program-arguments-and-entry) and
[validation evidence](arguments-validation.md).

## Environment and long paths

Forge owns `export KEY=value`, `unset KEY`, and `env`. Quotes allow values such as
`export TEST_ENV="hello world"`. The SDK copies an inherited environment into each
child; subsequent parent/child mutations are independent. `spawn_with_env` selects
an explicit environment. Portal forwards input and displays output; `cd`, parsing,
and environment policy remain shell responsibilities.

`cd`, `mkdir`, `ls`, `cat` and `medit` accept a quoted single path, including spaces.
`ls` uses the full-name directory ABI and displays VFAT long names. UTF-8 paths
must be shorter than 4096 bytes; a component allows up to 255 UTF-16 code units.
Short-name aliases remain accessible. See [current limits and evidence](capabilities-validation.md).
