# Repository instructions

Follow [the canonical repository instructions](../AGENTS.md), including mandatory
test-driven development and evidence before claiming completion. Read the nearest
scoped `AGENTS.md` for affected files. Start with [the documentation index](../docs/README.md),
[architecture](../docs/architecture.md), and [testing](../docs/testing.md).

`src/`, including `src/userspace/`, is Ring 0 kernel code. `user_programs/` and
`userlib` run in Ring 3. Preserve that boundary. Disclose failing or unrun checks;
never replace runtime evidence with compilation or a screenshot.

Keep detailed policy in `AGENTS.md` rather than duplicating it here.
