# QEMU end-to-end rules

Inherits [repository instructions](../AGENTS.md). Read [testing](../docs/testing.md).

- Use failing interaction assertions before implementing a visible behavior change.
  Build fresh user programs and the GRUB disk before running desktop tests.
- Assert results, not just a command echo: file contents, distinct command output,
  process transitions, focus ownership, cursor coordinates, or framebuffer pixels.
  Bound every wait; prefer waiting for explicit state over increasing arbitrary sleeps.
- Use the matching input path. Absolute pointer tests must inject absolute QMP events;
  relative monitor motion cannot validate Wayland-style absolute integration.
- Give each QEMU process unique temporary sockets/logs. Always terminate and reap
  QEMU in `finally`; preserve diagnostics on failure. Do not boot two writers against
  the same disk or rebuild an image still attached to QEMU.
- Screenshots are review artifacts, not sufficient pass criteria. Inspect them when
  graphics change. Test kernel↔SDK↔application behavior, not only serial boot banners.
- Prefer the current `desktop.py` for the desktop. Several older scripts have stale
  metrics or missing helpers; reproduce and report failures rather than claiming
  those scripts passed. Keep historical evidence distinct from current verification.
- Document manual host GUI checks separately; injected input does not prove COSMIC,
  GTK, Wayland, or X11 capture/integration works on a human's desktop.
