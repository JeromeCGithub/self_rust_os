# Initator M2: configured services and activation jobs

M2 replaces PID 1's compiled Glow/Flicker catalog with bounded native unit files,
whole-catalog validation, target/dependency jobs, ownership-aware rollback and a
host-tested process adapter. It keeps the M1 native process ABI and opt-in boot
profile. No kernel source, syscall numbers, toolchain or dependencies changed.
See the [unit interface](initator-units.md) and [complete roadmap](init-system-plan.md).

Branch `feat/initator-m2` is based on M1 commit
`394c1e027ad4408b0420fb8eeadce214cb8c8d5f`, in the isolated
`/home/jay/personal/self_rust_os-initator` worktree. M1 is
[PR #54](https://github.com/JeromeCGithub/self_rust_os/pull/54). M2 is a separate,
stacked milestone PR against `feat/initator-m1`. Nothing was merged; unrelated
changes in the original worktree remain separate.

## Behavior, boundary and decisions

- Ring 3 loads `/ETC/INIT/SYSTEM`, validates every file/reference/cycle before
  starting any daemon, then activates the explicit `boot` unit. Invalid input
  produces bounded diagnostics and leaves PID 1 alive in recovery mode.
- The native line parser uses fallible allocation and exact UTF-8/line errors.
  It preserves literal vectors, rejects unsupported mechanisms and bounds names,
  files, total bytes, vectors and graph edges. Cwd remains `/`; readiness remains
  Spawned. It is not a shell or TOML parser.
- The immutable catalog sorts unit names independently of FAT enumeration. Requires
  pulls in and waits for success; Wants pulls in optional jobs; After orders only
  selected jobs and waits for settlement, without requiring success. Mixed barrier
  cycles return actual closed paths. Pure Wants membership cycles are permitted.
- Serialized transactions coalesce duplicate pending starts and reject conflicts.
  Job IDs, launch run IDs and complete process keys reject stale callbacks. Root
  ownership preserves shared/preexisting services during failure cleanup. Cleanup
  uses reverse actual activation order and waits for child completion, rather than
  treating a termination request as an acknowledgement. Across retained roots,
  admission reserves at most 56 service slots.
- The shared Manager integrates transactions with the bounded M1 supervisor.
  Catalog slots start inactive. Stop/start cannot bypass rolling attempt limits,
  cancelled launches must drain, and terminal admission/rate-limit failures settle
  their jobs rather than leaving them pending forever.
- Native PID 1 drains child events between effects. Observed startup failures block
  dependents; retryable startup crashes hold them through backoff until replacement
  or the terminal limit. Already-running dependents have no implicit liveness
  binding after activation. An exit can race the next spawn; exclusive children
  launched in that interval are terminated and reaped before reporting job failure.
  No userspace check promises atomicity with a subsequent spawn syscall.
- The SDK adds `fs::read_dir_bounded`, sharing its transport/decoder with existing
  `read_dir`. Wire size is checked before allocation; names/results use fallible
  reservation, record limits are explicit, size races have four bounded retries,
  and malformed/truncated records never return a partial list. Kernel filesystem
  enumeration itself is unchanged and still allocates its existing listing.
- The native builder installs separate system definitions and user templates.
  Packaging validates source kinds, byte/count limits and FAT case collisions
  before mutating the image. Ordinary desktop packaging remains unchanged.
- App-specific mode-file policy left Initator. M1 crash tests now inject literal
  unit arguments; Zinit's private vector probe verifies actual argument/environment
  delivery without becoming an installed desktop application.

The native adapter currently activates one boot root. Other start/stop/conflict/
shared-root operations are public core APIs tested with mock process effects;
there is no external control endpoint until M3. Failed launch syscalls fail
activation immediately; the ABI does not classify transient/permanent errors.
Successfully spawned processes use configured restart policy. The historical
activation outcome is distinct from later runtime health.

## Red evidence and corrections

| Check added before implementation | Observed failure |
| --- | --- |
| Initial parser tests | Ten behavioral failures against inert parsing; later DEL/Unicode controls were incorrectly accepted |
| Initial graph tests | Seven assertions failed for missing validation/closure/cycle behavior |
| Transaction tests | Seven assertions failed for missing activation, concurrency, ownership and cleanup; a later stale job callback was wrongly accepted |
| Inactive supervisor tests | Unselected catalog entries launched; cancelled spawn reactivation was accepted |
| Manager adapter tests | Four initial tests produced no process effects; later exhausted admission left a job Running forever |
| Startup retry test | A retryable prerequisite exit failed the whole job immediately instead of holding its dependents through backoff |
| Directory transport/decoder tests | Four behavioral failures: byte budget ignored, no records decoded, malformed records accepted, no bounded retry behavior |
| Unit installer tests | Five invalid-source subcases failed to reject before writes; ordinary installation produced no commands |
| Configured/packaged QEMU tests | Fresh M1 manager ignored injected configuration, ran its compiled pair and never reported catalog/target results; [red trace](initator-m2/configured-red.log) |
| Catalog byte-budget diagnostic | Recovery omitted the reason for exceeding the total budget; now reports the offending file and `catalog-budget` |
| Prerequisite exit QEMU regression | Queued crash was drained only after launching its dependent and reporting success; [red trace](initator-m2/early-exit-red.log) |
| Literal-vector QEMU probe | Old Zinit rejected its service identity and exited before verifying the supplied vectors |

Initial standalone-directory visibility and test-helper naming compile errors were
corrected before recording behavioral red failures; they are not counted as red
behavior evidence.

Two test assumptions were corrected explicitly during validation. The 65th unit is
rejected by the loader's count check, so it now has a precise `unit-count` diagnostic,
while larger wire listings fail the SDK budget. At 32 CPUs, a prerequisite can exit
after the manager's last observation while the next spawn is underway. The guest
assertion now verifies either no dependent launch after observed completion, or
exactly one raced child with acknowledged rollback before Failed. It still rejects
the old manager's premature success. Host tests independently inject an already
observed exit and require zero dependent launch effects.

The M1 fault fixture can exhaust its three retries before boot activation settles;
M2 then reports Degraded rather than a post-boot unit Failed state. Both terminal
paths retain the same exact three-run, typed-fault, backoff, distinct-identity and
later surviving-Glow-heartbeat assertions. No timeout was increased or test skipped.

## Checks executed for M2

Host policy and SDK checks:

```bash
# Run outside the root bare-metal Cargo configuration.
cd /tmp
cargo +nightly-2025-01-30 test --manifest-path /home/jay/personal/self_rust_os-initator/crates/service-core/Cargo.toml --target x86_64-unknown-linux-gnu
```

[53 core cases passed](initator-m2/core-tests.log): 12 config, 9 graph, 12 transaction,
7 manager and 13 supervision. Coverage includes malformed/bounded configs, duplicate
aliases, cycles, independent pending starts, shared-root rollback, late siblings,
stale job/run/process identities, process loss during cleanup, rolling restart
storms, headroom release and required-target reopening during startup retries.

```bash
rustc +nightly-2025-01-30 --edition=2021 --test user_programs/userlib/src/fs/directory.rs -o /tmp/initator-directory-tests
/tmp/initator-directory-tests
python3 -B -m unittest discover -s tools/tests -p 'test_*.py'
bash -n tools/build-grub-disk.sh
```

Four standalone directory cases and [62 Python tooling cases](initator-m2/tools-tests.log)
passed. The Python suite includes installer failures and documentation-checker
regressions. The finalized documentation checker, its seven regression cases, and
`git diff --check` passed.

Builds succeeded: default kernel, GRUB kernel, GRUB release via both image builders,
native-init release, all user binaries/metadata, and `cargo test --no-run`. These
are compilation/artifact evidence, not runtime test passes. Runtime kernel checks
passed **91 library cases** and **6 selected integration cases**:

```bash
cargo test --lib
cargo test --test process_supervision --test native_supervision --test stdio --test threads --test display_mapping
```

Native images were freshly rebuilt with `RUSTOS_BOOT_PROFILE=native-init
./tools/build-grub-disk.sh target/init-m2.img`. Each guest used its own writable
copy. **56 distinct configured/supervision scenarios passed**:

| Runner/cases | CPU counts | Scenarios |
| --- | --- | --- |
| `init_units.py`: configured, packaged, cycle, unknown, malformed, required-failure, optional-failure, vectors, too-many, total-budget, oversize, encoding, service-budget, order-only | 1, 8, 32 | 42 |
| `init_units.py`: early-exit | 8, 32 | 2 |
| `init.py`: steady, two-services, fault-service, orphans | 1, 8, 32 | 12 |

Example commands:

```bash
RUSTOS_TEST_DISK=target/init-m2.img RUSTOS_CPUS=8 python3 -B e2e/init_units.py --case configured
RUSTOS_TEST_DISK=target/init-m2.img RUSTOS_CPUS=32 python3 -B e2e/init_units.py --case early-exit
RUSTOS_TEST_DISK=target/init-m2.img RUSTOS_CPUS=8 python3 -B e2e/init.py --case fault-service
```

A separate **native ABI scenario at 8 CPUs** also passed after regenerating the
updated test-only probe. Literal vector delivery was rerun at all three CPU counts
after that regeneration. Total unique native scenarios: **57**. Checks were resumed
at failures after corrections; there is no claim that all initially incorrect test
assumptions passed in one uninterrupted run.

Representative actual traces:
[ordered startup](initator-m2/configured-8.log),
[closed cycle](initator-m2/cycle-8.log),
[bad reference](initator-m2/unknown-8.log),
[unsupported readiness with location](initator-m2/malformed-8.log),
[required failure cleanup](initator-m2/required-failure-8.log),
[optional failure cleanup](initator-m2/optional-failure-8.log),
[vectors](initator-m2/vectors-8.log),
[early exit](initator-m2/early-exit-8.log),
[byte budget](initator-m2/total-budget-8.log),
[service admission budget](initator-m2/service-budget-8.log),
[After without success requirement](initator-m2/order-only-8.log).

The fresh default image `target/initator-m2-desktop.img` passed the complete
8-CPU [desktop E2E](initator-m2/desktop-e2e.log): compositing, launchers, shell/file
commands, argument handling, concurrent apps, focus, dragging, close and compositor
survival after DE exit. Its [fresh screenshot](screenshots/initator-m2-desktop.png)
was inspected; it shows desktop compatibility, not a native graphical session.
Native supervision is demonstrated by process/effect traces instead of a screenshot.

## Remaining milestones and limits

No M2 acceptance check remains failing. Trusted FAT image input is still required;
there are no user identities, filesystem ACLs, descendant quotas, readiness IPC,
watchdogs, management endpoint, reload, graceful stop, poweroff or Nest sessions.
PID 1 uses bounded synchronous file reads/spawns; launch-latency measurement and a
worker if justified belong to M3. M5 provides enforcement rather than relying on
configuration conventions. Default desktop boot remains the legacy PID 0 path until
M6. The user template does not claim that a session manager exists.

Manual host GUI behavior was not tested; QMP interaction does not establish human
pointer/display integration. The inherited pinned bootloader warning remains.
Unrelated historical heap-stress/gfx-demo limitations were not rerun or hidden by
this milestone; M1 fatal-init/exhaustion matrices remain evidence for their earlier
kernel commit, not fresh M2 execution claims. No screenshot or log substitutes for
those omitted scenarios.
