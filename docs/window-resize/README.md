# Window resize evidence

See [implementation, contract, commands and limitations](../window-resize.md).
Logs and screenshots here come from this work's actual host/QEMU runs.

- Red: [old protocol rejects larger geometry](protocol-red.log),
  [geometry does not resize](geometry-red.log), [cache missing](cache-red.log),
  [native expander leaves wallpaper pixels](native-red.log).
- [Form focus lost across maximize](form-red.log) preceded compositor decoration
  capture. The original exact query assertion remains in the green run.
- [Maximized shadow shades the reserved panels](panels-red.log) preceded removing
  shadows from committed maximized windows. New pixel assertions include panel
  rows immediately outside the window, not only the outer display edges.
- [Expansion allocation/refusal and in-flight mode red](expansion-red.log),
  [reservation/in-flight geometry red](reservation-red.log),
  [5 geometry/mode tests green](geometry-green.log).
- [27 SDK raster/input/protocol tests](ui-green.log), [cache bounds](cache-green.log),
  [5 blur/material tests](glass-green.log), [3 pointer-input tests](input-green.log),
  [5 damage tests](damage-green.log).
- [8-CPU browser and real CERN run](native-green.log): grow/shrink, maximize/restore,
  controls beyond x1023, preserved form queries, scripts, images/history/fonts,
  HTTP/TLS cancellation, Terminal survival and relaunch. This precedes the final
  pure expansion-state/allocation-publication refactor; the final native glass run
  below exercises those shared compositor/SDK paths after refactoring.
- [Final 8-CPU native glass](glass-native-final.log): repeated maximize/restore,
  exact panel pixels, preserved declarative widget state, release capture, drag,
  close/reopen, focus and live lower-layer changes.
- [Desktop regression](desktop-green.log), [native display mapping](display-mapping-green.log),
  [45 host tooling checks](tools-green.log). Default/GRUB/release builds and complete
  kernel test compilation also passed; these are compilation evidence.
- [Version-1 Hello compatibility](legacy-green.log) used unchanged HELLO.BIN from
  commit `b2a5167` in a separate disposable copy of the new-compositor disk. The
  actual toolkit/glass/button/focus suite passes with the original wire format.
- The performance runner initially [launched Terminal instead of Snake](perf-launcher-red.log)
  because its coordinates predated browser installation. Its [host regression](perf-host-red.log)
  preceded using actual DE-discovered launcher positions. [Host green](perf-host-green.log),
  [performance green](perf-final.log): unchanged15% idle/200ms pointer/150ms keyboard
  and loaded-input assertions pass. Final measured idle desktop0.7%, terminal7.5%,
  pointer maximum30ms, keyboard drain0ms, loaded pointer19ms.
- The longer browser run exposed two observation problems, retained as failures:
  [fixture expired while typing the cancellation URL](cancellation-fixture-red.log)
  and [typing began before the field's focus frame](focus-observation-red.log).
  Cancellation now uses existing page links for a prompt superseding navigation;
  the same socket-closure and deadline assertions remain. Form typing waits for
  the actual blue focus border. No timeout, exact query or recovery assertion was
  relaxed. These improvements pass in the full browser run above.

The screenshots were captured from those tested builds and inspected:
[maximized browser](maximized-browser.png), [edge-resized browser](resized-browser.png),
[restored browser](restored-browser.png), [edited form maximized](maximized-form.png),
[real CERN page maximized](cern-maximized.png), [maximized glass](glass-maximized.png).
They supplement assertions; manual host GUI/input remains untested.
