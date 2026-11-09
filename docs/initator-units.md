# Initator native units, version 1

The opt-in `native-init` boot profile loads `/ETC/INIT/SYSTEM/*.UNIT`, validates
one immutable catalog, then activates the unit named `boot`. Configuration and
job policy execute in Ring 3. The kernel sees explicit executable paths, literal
launch vectors, exact process identities, child events and deadline waits.

The [roadmap](init-system-plan.md) separates the implemented M2 catalog from
M3 readiness/IPC work and future login, user sessions and graphical supervision.
[M2 evidence](initator-m2-progress.md) records its validation; the
[M3 progress record](initator-m3-progress.md) distinguishes tested additions from
remaining work. [M1 evidence](initator-progress.md) describes the earlier
compiled catalog and the unchanged native child ABI.

## Files and fields

System defaults are in [config/init/system](../config/init/system/BOOT.UNIT).
The installer also creates `/ETC/INIT/USER`; its `session` target is a template,
not a running user manager. The default desktop profile does not install these
units or change its WM boot path. Normal `.BIN` applications are never scanned
into the catalog. Service executables remain `.SVC` files outside desktop launchers.

```ini
Version=1
Kind=service
Name=lantern
Exec=/BIN/LANTERN.SVC
Arg=--foreground
Arg=two words in one argument
Arg=
Env=MODE=system
WorkingDirectory=/
Ready=spawned
Restart=on-failure
RestartDelayTicks=18
RestartMaxDelayTicks=180
RestartBurst=3
RestartWindowTicks=1800
HealthyResetTicks=1800
```

A target has no process:

```ini
Version=1
Kind=target
Name=boot
Requires=lantern
Wants=chime
```

| Field | Meaning |
| --- | --- |
| `Version` | Required, exactly `1` |
| `Kind` | Required, `service` or `target` |
| `Name` | Required unique ASCII identifier, at most 48 bytes; lower-case letters/digits initially, then letters/digits/`-_.` |
| `Exec` | Required for services, explicit absolute path; no search, `//`, `.` or `..` components |
| `Arg` | Repeated literal arguments after the executable; empty values allowed |
| `Env` | Repeated `NAME=value`; distinct identifier keys, explicit values, no manager environment inheritance |
| `Requires` | Pulls in another unit and requires its successful activation before this job |
| `Wants` | Pulls in another unit; failure can degrade the target without failing it |
| `After` | Orders selected jobs only; never pulls a unit in or implies its success |
| `Ready` | `spawned` (default) or `notify`; notify waits for the exact run's authenticated Ready message |
| `StartTimeoutTicks` | Positive deadline from launch reservation, default 90; includes synchronous loading and notify initialization |
| `WatchdogTicks` | Optional positive interval; requires `Ready=notify`, starts at Ready and refreshes only on valid work-path Heartbeat |
| `WorkingDirectory` | Only `/`, the default, is supported until atomic per-launch cwd exists |
| `Restart` | `never` (default), `on-failure`, or `always` |
| `RestartDelayTicks` | Positive initial backoff, default 18 |
| `RestartMaxDelayTicks` | Backoff cap, at least initial delay, default 180 |
| `RestartBurst` | Rolling-window attempt limit, 1–32, default 3 |
| `RestartWindowTicks` | Positive rolling attempt window, default 1800 |
| `HealthyResetTicks` | Positive run duration that resets failure history, default 1800 |

Targets reject service-only fields regardless of field order. Dependencies and
vectors repeat; singleton fields, duplicate edges within a kind, and duplicate
environment names are rejected. `Requires=a` together with `After=a` is valid.

Values follow the first `=`. Outer ASCII whitespace is trimmed, interior spaces
are literal; UTF-8 is supported in arguments/values/paths. CRLF, blank lines and
full-line `#` comments work. Control characters, unknown keys, malformed numbers,
unsupported readiness modes, stop timeouts and other cwd values are rejected.
Timing fields must fit `1..=i64::MAX`; deadline arithmetic is checked.
There is no shell quoting, expansion, interpolation, inline-comment language or
command substitution. For example `Arg=$(literal)*` is one unchanged argument.
Do not put secrets in diagnostic unit names; launch vectors are not logged.

Limits apply before activation: 64 catalog units, 256 total dependency/order edges,
8192 bytes per file, 128 KiB total file bytes, 32 arguments, 32 environment entries,
4096 combined executable/vector bytes. Filenames are distinct case-insensitively,
control-free basenames ending in `.UNIT`, at most 255 UTF-8 bytes. Unit names are
independent of filenames. All directory entries other than FAT `.`/`..` must be
regular unit files; unexpected files/directories fail the catalog. Directory reads
have separate wire-byte and record limits checked before userspace allocation.
File descriptors close on all paths, reads reject oversized files and size races,
and source/vector allocations use fallible reservation.

## Activation and ownership

The [parser](../crates/service-core/src/config.rs),
[catalog](../crates/service-core/src/graph.rs),
[transaction engine](../crates/service-core/src/transaction.rs) and
[manager adapter](../crates/service-core/src/manager.rs) contain no syscalls.
Catalog indices follow sorted names, independent of filesystem enumeration.

Validation covers the entire catalog, including unselected units. Unknown
references, duplicate definitions and cycles in the combined `Requires`/`After`
barriers fail before any service spawn. Cycle diagnostics give an actual closed
path. Pure `Wants` membership cycles are allowed; self edges are rejected.
Activation takes the `Requires`/`Wants` closure, orders only selected barriers,
and permits independent starts to be outstanding simultaneously.

One transaction runs at a time. Duplicate pending starts of the same root coalesce;
conflicting requests return Busy. Job IDs never wrap, and completion callbacks
must carry the matching job ID. Every successful root retains its own desired
closure, so rollback/stop of one root preserves services retained by another.
There is a global admission limit of 56 services across retained roots, leaving
kernel capacity for recovery and transient work. This admission budget does not
enforce descendants' memory/process limits; capability/resource enforcement is M5.

A missing executable or other failed spawn fails that activation immediately;
there is no child to reap. Spawn errors currently lack a typed transient/permanent
errno, so retrying failed launch syscalls during activation is deferred. A process
that was successfully spawned can restart according to its policy. If its exit is
observed while activation is still pending, dependents wait through retry backoff;
terminal failure blocks them. The native loop drains child events between effects.
With `Ready=spawned`, readiness means successful spawn. With `Ready=notify`,
the service remains Starting until an authenticated Ready for its current run.
An exit immediately after an observation is always possible.

Optional failures yield Degraded; required failures yield Failed. Cleanup stops
only exclusively acquired services, in reverse actual activation order, and waits
for exact child completions before finishing the job. Failed optional groups do
not retain their exclusively started prerequisites. Post-activation `Requires`
is not a liveness binding: an already-running dependent does not stop merely
because its prerequisite subsequently crashes. The recorded activation outcome
is historical; it is not a continuously recomputed system-health assertion.

Explicit stops suppress restart before terminating the process. Launch tokens,
process generations and job IDs reject stale results. Stop/start preserves the
rolling attempt history. Restart deadlines use monotonic native ticks, with capped
exponential backoff. PID 1 waits on durable child readiness or the earliest deadline;
there is no periodic service polling or asynchronous runtime.

## Failures and current control limits

Invalid catalogs print bounded file/line/key or graph diagnostics and enter recovery
mode with PID 1 alive and no services launched. Recovery has no reload channel yet. Valid but unsuccessful boot
activation reports Failed/Degraded and keeps supervising retained services.

`Manager::start`/`stop` and status APIs currently serve the native boot adapter and
host tests; no public IPC control endpoint exists. Reload, notify readiness,
watchdogs, cooperative stop, shutdown/poweroff, durable logs, user identities and
Nest sessions remain future milestones. M2 adds no syscall numbers or privileges.
FAT configuration is trusted image input until access enforcement exists; limits
bound PID 1's own allocations but do not redesign kernel filesystem enumeration.


## Explicit readiness and watchdogs (M3 work in progress)

A notifying service uses `userlib::service::Client::bootstrap()` to take its
nondelegable notify and optional log grants. It dispatches native port events to
`on_event` until Init establishes its run. It calls `ready()` after initialization,
then `heartbeat()` only after completed work when a watchdog is configured.
A failed send does not commit client readiness; retry explicitly on writable
interest. No background pinger makes a stalled service look healthy.

The manager sends Init after registering the child's process identity and per-run
endpoints. A payload run number cannot confer authority: the receiving endpoint,
kernel-supplied sender, exact process identity and current run must all agree.
Ready at or after the startup deadline is late; duplicate Ready does not refresh
a watchdog. Heartbeat is invalid before Ready or without a configured watchdog.
Logs use a separate channel and cannot authorize readiness.

Startup/watchdog failure withdraws readiness and requests one termination for the
owned process. Child completion acknowledges cleanup; issuing kill does not.
An exit before Ready, including status zero, counts as failed initialization.
Restart backoff/budgets still apply; healthy-history reset begins at readiness.
Requires orders activation but is not a continuing liveness binding: an already
active dependent survives prerequisite restart. Compositor/session failure policy
requires the later explicit session lifecycle work.

Native logging is a bounded volatile 256-record journal, not stdout capture or
persistent storage. Records contain authenticated unit/run/process attribution,
monotonic tick, severity and at most 232 UTF-8 bytes without control characters.
Overwritten history reports a cursor gap. Per-run log/notify budgets pause receive
interest and resume on deadlines; logging performs no disk or console output.
Spark query/control integration is still pending; see the M3 progress record.
