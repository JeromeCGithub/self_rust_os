## Problem and behavior

Describe the trigger, before/after behavior, acceptance criteria, and affected
kernel/userspace boundary. Link related issues and docs.

## TDD evidence

- Red: test added first, exact command, and observed failure for the expected reason.
- Green: implementation and exact passing checks, including executed cases/behaviors.
- Refactor: changes made while retaining passing tests, if applicable.

For prose-only changes, include documentation-check evidence instead of inventing
runtime tests. For tooling changes, include executable regression tests.

## Validation

- [ ] Read root and scoped `AGENTS.md`; ran the union of applicable testing-matrix rows.
- [ ] Added success/error/boundary/lifecycle coverage appropriate to the change.
- [ ] Rebuilt dependent userspace binaries/metadata and disk if applicable.
- [ ] Updated architecture/ABI/build docs for changed contracts.
- [ ] Reviewed final diff, generated artifacts, and screenshots when applicable.
- [ ] Recorded baseline failures, blocked/unrun checks, and manual verification below.

List commands and actual results. Builds and `--no-run` are compilation evidence only.
Do not check a box for a test that did not run or a behavior that was not observed.

## Limitations and remaining verification

State failure/cleanup, unsafe/ABI, compatibility, resource, and host GUI risks as relevant.
Never imply all tests passed when a required check remains failing or unrun.
