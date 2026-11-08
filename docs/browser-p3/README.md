# P3 execution evidence

See [architecture, API profile, budgets and open gates](../browser-javascript-p3.md).
All logs are actual executed host/guest runs from this phase; tests supplement
screenshots. No manual host GUI capture or modern-site compatibility is implied.

- [Classic completion/promise jobs red](jobs-red.log), [host call gate red](host-red.log),
  [native-adapter engine green](engine-native-green.log), [libc engine green](engine-libc-green.log).
- [DOM effects red](effects-red.log), [effects green](effects-green.log),
  [native DOM red](dom-red.log), [native-adapter DOM green](dom-native-green.log),
  [libc DOM green](dom-libc-green.log), [browser regressions](browser-green.log).
- [Document IPC red](protocol-red.log), [protocol green](protocol-green.log).
- [Old application misses native DOM/promise title](native-red.log),
  [old worker rejects Document request](worker-red.log).
- [Connected host cannot attach worker](nested-red.log),
  [already-exited status overwritten during new cleanup implementation](exit-status-red.log).
  Both regressions pass in [final kernel integrations](kernel-integration.log).
- [Global native allocation-counter fixture race](counter-red.log): adding a second
  lifecycle test exposed shared global fixture overlap. The two fixtures now take
  the same test mutex; default parallel Cargo tests pass without relaxing the
  zero-allocation assertion or forcing the runner into single-thread mode.
- [90 kernel units](kernel-unit.log), [5 release heap cases](heap-green.log),
  [44 tooling cases](tools-green.log). Default/GRUB/release builds and complete test
  compilation also passed; build output is not counted as a runtime test.
- Restricted/native/nested ownership and exact frame-recovery probes:
  [1 CPU](worker-1cpu.log), [serial](worker-1cpu-serial.log);
  [8 CPUs](worker-8cpu.log), [serial](worker-8cpu-serial.log);
  [32 CPUs](worker-32cpu.log), [serial](worker-32cpu-serial.log).
  These preceded the final constant-stack module refactor/initialized-FFI-buffer
  audit; the post-refactor native runs are recorded separately below.
- [Current live Google failure](google.log), rerun on2026-11-06 after native DOM
  integration: default homepage/query submission reached the real response, but
  strict usable-results checking still fails on unsupported JavaScript/APIs.
  Neither a homepage nor the fallback is counted as search results.

The [native script screenshot](native-script.png) shows actual class/function/
array/promise-driven DOM text `Native functions: 82,84` and changed colors in jbrowse.
The [queue-limit screenshot](native-queue.png) shows the readable document retained
after an infinitely replenished microtask queue was bounded. No host renderer or
site-specific object/data substitution is used.

Final source verification after the channel-module refactor and initialized-FFI-
buffer audit:

- [32-CPU direct/nested native service](worker-final.log),
  [complete serial assertions](worker-final-serial.log).
- [8-CPU full browser](native-green.log), [serial](browser-serial.log): native
  classes/functions/promises and DOM pixels, bounded microtasks, previous scripting
  fixtures, PNG/alpha/history, typography, forms, Stop, HTTP/TLS cancellation,
  Terminal survival and relaunch.
- [8-CPU desktop](desktop-green.log): independent applications, focus/drag/close,
  editing, concurrency and compositor survival. The generic screenshots generated
  by this suite were restored; phase-specific evidence is retained here.
- Host final-source results:9 native-adapter engine/lifecycle cases,7 libc engine
  cases,4 DOM cases under each adapter,2 effect-codec cases,4 IPC cases,23 browser
  cases and44 tooling cases. The allocation counter tests pass under the ordinary
  parallel runner with their shared fixture mutex.

![Actual native JS/promise-driven document](native-script.png)

![Readable page after a bounded microtask loop](native-queue.png)

The final Google failure was captured before the constant-stack module refactor
and initialized-output-buffer audit; neither change supplies missing browser APIs.
It is still a failure, not an unexecuted or inferred pass. Full Test262/WPT,
manual host GUI/input, broad modern sites and the complete P3 exit gate remain open.
