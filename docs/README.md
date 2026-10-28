# Project documentation

Read these before changing the OS. The source and reproducible tests determine
current behavior; architecture targets are not evidence of implemented features.

| Document | Purpose |
| --- | --- |
| [AI contribution instructions](../AGENTS.md) | Mandatory TDD, verification, safety, and delivery rules |
| [Architecture](architecture.md) | Ring 0/Ring 3 ownership, repository map, lifecycle, and limitations |
| [Development](development.md) | Toolchain, build artifacts, red/green/refactor workflow, PR expectations |
| [Testing](testing.md) | Change-to-test matrix, commands, evidence requirements, baseline gaps |
| [Syscall ABI](syscalls.md) | Current interface between kernel and userspace |
| [GRUB boot](grub-boot.md) | Current disk layout and boot sequence |

Scoped instructions: [kernel](../src/AGENTS.md), [applications/SDK](../user_programs/AGENTS.md),
[kernel tests](../tests/AGENTS.md), and [QEMU E2E](../e2e/AGENTS.md).

[Filesystem review](arch/fs-hierarchy-review.md) is historical context from an older
PR. Re-check its claims against the current implementation before applying them.

[Claude entry point](../CLAUDE.md) and [Copilot instructions](../.github/copilot-instructions.md)
point back to the same policy. The [PR template](../.github/pull_request_template.md)
requests red/green evidence; [documentation CI](../.github/workflows/documentation.yml)
runs the checker and its regression tests. Instructions express required behavior;
CI checks links and entry points, not whether an agent truly followed TDD.
