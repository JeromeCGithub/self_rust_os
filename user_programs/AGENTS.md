# Ring 3 application and SDK rules

Inherits [repository instructions](../AGENTS.md). These are separate `no_std`
program crates, not a Cargo workspace. The SDK in `userlib/` also runs in Ring 3.
Read [architecture](../docs/architecture.md) and [ABI](../docs/syscalls.md).

- Use typed `userlib::sys` calls. Do not import `self_rust_os`, access hardware ports,
  manipulate page tables, or rely on privileged kernel addresses.
- Put command parsing, PATH/search decisions, application discovery, and desktop
  presentation here. The kernel loads the explicit path supplied to `spawn`.
- The WM owns physical display presentation. Other apps draw into private mapped
  surfaces; graphical clients call `graphics_mode()` so console diagnostics do not
  overwrite their drawing. Query geometry; clip drawing and honor pitch.
- Only the focused app consumes keyboard input. Keep key release/modifier handling
  correct across reads. Test actual pointer motion, buttons, focus, and drag events;
  seeing an emulated mouse device does not prove host input reaches the guest.
- Use `userlib`'s runtime; programs define `user_main`, not a second `_start` or
  panic handler. Preserve linker entry/base, stack bounds, and flat-binary format.
- TDD must cover shell commands, long/empty input, scrolling, filesystem errors,
  failed launches, and exit behavior as relevant. Exercise independent simultaneous
  applications when changing shared SDK, compositor, or scheduling interfaces.
- Run `./tools/build-user.sh` and rebuild the GRUB disk before runtime tests. Run
  `python3 e2e/desktop.py` and applicable kernel/ABI tests. Inspect screenshots for
  graphics work and disclose that QMP injection is not a manual host desktop test.
- Commit regenerated tracked `.bin`/`_meta.rs` with their source changes. Do not
  manually patch either or rely on stale artifacts to make a build pass.
