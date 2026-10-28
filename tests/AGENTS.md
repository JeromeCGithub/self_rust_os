# Kernel test rules

Inherits [repository instructions](../AGENTS.md). Read [testing](../docs/testing.md).
These tests boot kernels in QEMU using Rust's custom `#[test_case]` framework;
they are not ordinary host `#[test]` binaries.

- Write an observable failing regression before changing behavior. Include boundary
  and error cases. A constants-only assertion is not coverage of a syscall transition.
- Use RAM-backed framebuffers from `common/` for legacy integration tests. Initialize
  the heap before `alloc`; initialize queues before injecting input. Explicitly set
  test-specific IRQ masks rather than depending on desktop boot sequencing.
- Exit through the project's QEMU debug-exit protocol. Expected panic/stack-overflow
  tests must distinguish the expected failure from arbitrary faults or timeouts.
- `gfx_demo` is an interactive nonterminating demo, not a passing automated test.
  Do not weaken assertions or label a timeout as success.
- ATA tests mutate the primary slave fixture; FAT32 tests mutate the secondary master
  fixture. Never use the boot disk or host disks as writable fixtures. Run tests
  sharing writable fixtures sequentially or provision independent copies.
- Preserve test exit codes and pipeline failures. Record which cases executed;
  `cargo test --no-run` proves only compilation.
