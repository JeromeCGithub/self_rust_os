# Kernel rules

Inherits [repository instructions](../AGENTS.md). Everything below `src/` executes
in Ring 0; `src/userspace/` is kernel-side process loading and syscall handling.
Read [architecture](../docs/architecture.md) and [ABI](../docs/syscalls.md).

- Keep desktop/shell policy in `user_programs/`. Drivers expose mechanisms, not app names.
- For memory or syscall changes, write failing tests for invalid/unmapped/read-only
  buffers, overflow, page boundaries, failure rollback, and process isolation as relevant.
  Never dereference a user pointer solely because it is numerically in the user range.
- For scheduler/interrupt changes, prove two processes make progress, focused input
  reaches only its owner, blocking/wakeup preserves return registers, and exit/kill/wait
  cannot resume a dead context. Test kernel-mode IRQs separately from user preemption.
- For drivers/parsers, test malformed/truncated data and missing hardware; bound retries.
  Preserve EOI behavior, IRQ masks, and boot ordering. Do not unmask the timer before
  a valid scheduler context exists.
- Run both boot-path builds, kernel runtime tests, and the desktop E2E for affected
  boundaries. Consult [known test gaps](../docs/testing.md#known-baseline-gaps);
  `tests/userspace.rs` constants do not demonstrate actual Ring 3 execution.
- Keep existing tests' RAM framebuffer backing separate from physical display MMIO.
  A kernel test must not draw through an assumed physical-memory offset.
- Parse disk/boot wire formats with explicit widths, byte order, bounds and unaligned
  access rules; do not create Rust references to unaligned packed fields. Use explicit
  representation for assembly/ABI structs, with layout tests where offsets matter.
- Do not introduce floating point, MMX, or SIMD use without implementing and testing
  CPU enablement and context preservation. Respect target features and red-zone rules.
  Keep kernel-stack usage bounded; large buffers belong in owned mapped storage.
