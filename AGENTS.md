# Instructions for AI contributors

These rules apply throughout this repository. Read the nearest nested `AGENTS.md`
as well. Use this exact filename so coding agents discover it automatically.

## Start here

1. Read [the documentation index](docs/README.md), [architecture](docs/architecture.md),
   [development workflow](docs/development.md), and [testing guide](docs/testing.md).
2. Inspect `git status`, the affected source, its callers, and its tests before editing.
   Preserve unrelated work. Verify the current branch and base before a PR.
3. State the requested behavior, affected privilege boundary, acceptance criteria,
   and tests that will prove it. Make small, reviewable changes.
4. Treat source, toolchain/configuration, and reproducible execution as evidence.
   Correct stale docs; do not implement a historical document as if it were current.

## Mandatory test-driven development

- **Every behavior change follows red → green → refactor.** First add or strengthen
  a test expressing the requested behavior. Run it and confirm it fails for the
  expected missing behavior, not a broken environment. Then implement the smallest
  change, rerun the test, and refactor only while it stays green.
- Reproduce a bug with a regression test before fixing it. Cover success, failure,
  boundary values, and cleanup/recovery wherever applicable. Test observable
  behavior and invariants rather than duplicating implementation expressions.
- Hardware, assembly, boot, and graphical changes still require TDD: use QEMU
  interaction, serial assertions, fault injection, or framebuffer readback when
  ordinary unit tests cannot observe the behavior. Manual inspection supplements
  automated checks; it does not replace them.
- Documentation-only changes require documentation checks; changes to tooling
  require executable regression tests first. Do not invent runtime tests for prose.
- Run **all tests applicable to every changed subsystem and its integration paths**
  before reporting completion, following the matrix in [testing](docs/testing.md).
  Compilation, `--no-run`, a screenshot, or a log message alone is not a test pass.
- Establish the baseline for failing suites. Never delete, skip, weaken, change an
  expected value, increase a timeout, or suppress a failure merely to obtain green.
  Existing failures must remain visible with reproduction details. Fix failures
  caused by this work; do not silently expand a documentation task into kernel work.
- If a required check cannot run or remains failing, say what is incomplete and why.
  Do not describe the behavior as verified or the work as fully done. A PR must
  disclose failures and untested acceptance criteria, including host GUI behavior.

## Kernel versus userspace

Use a Linux-like **privilege and responsibility split**, not a claim of Linux ABI
compatibility. All of `src/`, including `src/userspace/`, is privileged kernel code.
`user_programs/` contains Ring 3 applications and their `userlib` SDK.

- Kernel: interrupts, drivers, memory protection, scheduling, resource ownership,
  filesystem mechanisms, syscall validation, and enforcement of access rules.
- Userspace: shell parsing, executable search policy, desktop layout, wallpaper,
  application discovery, window stacking, and rendering policy.
- Cross this boundary through the documented syscall ABI and typed `userlib`
  wrappers. Never import kernel internals into an application or execute app policy
  inside the kernel just to avoid designing an interface.
- Record existing exceptions honestly (PID 0 privileges, initial WM path, flat
  binaries). Do not describe desired isolation or POSIX facilities as implemented.

## Rust and OS correctness rules

- Keep `no_std` code compatible with the pinned nightly and custom targets. Do not
  update the toolchain, dependencies, target JSON, or linker layout incidentally.
- Minimize `unsafe`. Every new unsafe block needs a concrete `SAFETY` argument for
  validity, bounds, alignment, lifetime, aliasing, ownership, and concurrency as
  relevant. An address-range comparison alone does not prove a user pointer is mapped.
- Check arithmetic and lengths before forming pointers or slices. Validate complete
  buffers, page permissions, canonical addresses, wraparound, zero lengths, and
  supervisor/user access. Never retain user references across scheduling or unmapping.
- Distinguish physical addresses, virtual addresses, framebuffer offsets, disk LBAs,
  bytes, pages, and sectors in names and APIs. Derive geometry from boot/syscall data;
  honor framebuffer pitch, pixel format, page alignment, and MMIO caching rules.
- Page-table changes need explicit ownership and TLB handling. Preserve supervisor
  mappings and guard regions. Prevent aliasing between process address spaces; test
  cross-process isolation and failed allocation rollback. Do not claim W^X unless
  writable/executable flags actually enforce it on every relevant mapping.
- IRQ handlers must have bounded work. Avoid allocation, blocking, filesystem access,
  and locks that interrupted code may hold. Define lock order and interrupt state;
  disabling interrupts is not a substitute for future SMP synchronization.
- Preserve interrupt/syscall register order, stack alignment, ABI clobbers, GDT/TSS
  selectors, and `iretq` frames. Keep timer and syscall saved contexts consistent.
  Never disable preemption as a permanent fix for a context-switch bug.
- Treat the kernel as single-CPU unless explicitly implementing and testing SMP.
  Explain atomic ordering and shared-state synchronization; do not assume atomics
  alone make a multi-field operation safe.
- Bound device polling. Handle absent hardware, malformed input, short reads,
  disk-full and allocation failures. Return errors at trust boundaries; reserve
  panics for unrecoverable kernel invariants and boot prerequisites.
- Account for lifecycles: partial initialization, spawn failure, exit, kill, wait,
  descriptors, frames, stacks, and surfaces. Add repeated-operation tests for leaks
  and stale state. Do not turn current leaks into an assumed resource model.
- Keep serial diagnostics usable during faults, including when another lock is held.
  Avoid logging every pixel/event or leaking kernel/user data through diagnostics.
- Storage tests operate only on explicit disposable image files. Never format a host
  disk, mounted device, or unrelated image. Builders overwrite development images;
  tests must not race on writable fixtures or rewrite an image while QEMU uses it.
- Prefer small typed interfaces and explicit errors. Document public APIs and ABI
  changes. Do not add blanket lint allowances, unexplained `unwrap`, or placeholder
  `todo!` paths in implemented behavior.

## Build artifacts, documentation, and delivery

- Rebuild userspace with `tools/build-user.sh`; regenerate `.bin` and `_meta.rs`
  together. Never hand-edit generated binaries/metadata or commit `target/` images.
- An ABI change requires kernel dispatcher + SDK wrappers + callers + docs + tests
  in the same change. Preserve existing syscall numbers unless migration is explicit.
- Keep links and instructions current. Historical reviews in `docs/arch/` describe
  past decisions, not the current implementation contract.
- Validate changes, then inspect `git diff --check`, `git diff`, and `git status`.
  Never silently reset user work, rewrite shared history, merge, or publish beyond
  the user's request. Local reversible investigation does not require repeated approval.
- Completion/PR reports must include: behavior changed, red test and observed failure,
  green checks actually run, remaining failures/limitations, and manual checks still
  needed. Distinguish commands run now from evidence inherited from previous work.
