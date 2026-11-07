# P3: native JavaScript and initial DOM bindings

P3 is **in progress**. jbrowse now uses real QuickJS in its restricted Ring3
worker, replacing the straight-line prototype in the application. It can execute
classes/private fields, closures/functions, arrays/objects and promises while
mutating a document through bounded native calls. This is an initial binding
profile, not completion of the [P3 browser API/session gate](browser-support-plan.md).

## Ownership and execution

[`rustos-browser-dom`](../crates/browser-dom/src/lib.rs) owns one parsed document
and a fresh thread-confined QuickJS realm. The worker receives HTML only after
its irreversible P1 restriction handshake. Inline classic scripts run in source
order after parsing, sharing lexical/global state within that document. Each
script is followed by an explicit microtask checkpoint. External and module
scripts report unsupported failures; non-JavaScript data scripts are ignored.
There is no parser-blocking/defer/async implementation yet.

The [C bridge](../crates/quickjs/c/bridge.c) executes unused completion values
without converting them to strings; page-controlled `toString` is not invoked.
A checkpoint bounds both engine CPU polls and the number of jobs. A looping job
or infinitely replenished queue cannot monopolize the browser UI. A non-quiescent
checkpoint destroys that realm with its pending queue. Promise rejection tracking
and browser unhandled-rejection events are not implemented.

[`HostRuntime`](../crates/quickjs/src/host.rs) owns the runtime before its boxed
host, so engine destruction precedes native document destruction. It inherits the
runtime's creator-thread-only/no-Send/no-Sync contract. The C gate retains only a
stable host address and callback. Input strings are bounded/copy-checked UTF-8;
owned text/IDs/null/unit results become engine-owned values before returning.
No JSValue, native reference, kernel address, borrowed caller buffer or syscall
handle crosses the binding/wire interface. Host code cannot reenter its runtime.
The callback and FFI calls carry explicit safety arguments.

The trusted [bootstrap](../crates/browser-dom/src/bindings.js) captures and removes
the native call gate before page code. Wrappers cache identity and use private
node IDs; detached nodes remain alive with their arena until realm destruction,
but `getElementById` traverses only the connected tree. Text writes remain literal,
so markup is not parsed as a side effect. The initial audited interface surface is:

| Interface | Supported subset | Explicit gaps |
| --- | --- | --- |
| Document | body, title, getElementById | Full prototype hierarchy/brand checks, URL/location, documentElement, querying/tree/attribute mutation, title-element reflection |
| Element | Stable wrapper identity, textContent, style | Node creation/insertion/removal, innerHTML, form/focus/event integration, layout measurement |
| CSSStyleDeclaration | color, backgroundColor, display, fontSize/family/weight, width/height | Complete CSSOM and computed style, standards parsing/invalid-value behavior |
| Window | window/self refer to the same realm | Events/tasks/timers, animation callbacks, navigation/history APIs and other browser globals |

This hand-audited bootstrap is not generated complete Web IDL. Reference contracts:
[DOM textContent/getElementById](https://dom.spec.whatwg.org/),
[HTML Document accessors](https://html.spec.whatwg.org/multipage/dom.html),
[QuickJS embedding](https://bellard.org/quickjs/quickjs.html).
No site names, fabricated results or special Google/Facebook/YouTube objects exist.

## Document effects and limits

The browser process owns URL/UI/HTTP/CSS/image policy and painting. It has no
QuickJS/prototype dependency. The restricted service has no network/filesystem/
spawn/display authority after sealing. It returns pointer-free
[`Changes`](../crates/browser/src/dom.rs): title, textContent or a supported style
property for an arena index. The controller validates the whole record and applies
it to a bounded document copy before publishing any effects. A malformed record,
foreign index, invalid UTF-8, unknown operation, truncation, trailing bytes or
style-declaration injection cannot partially commit a reply. Single setter failure
also preserves the previous native document/log.

The P1 wire adds `Document=4`; header/version/sequence/reserved/status checks stay
in place. Document input is at most128 KiB, replies at most8192 bytes and256 effects.
`JDM1` records contain a magic, u32 failure count, u16 effect count, zero u16 reserved,
then operation/u32 node/u16 property length/u16 value length and UTF-8 strings.
P3 initially used a256 KiB static HTML limit. The current
[document envelope](browser-css-comparison.md) is2 MiB; executable-document
requests above128 KiB still fail the worker profile explicitly. Non-executable
data-script blocks do not start a worker merely because they contain large data.
At most64 inline scripts/128 KiB aggregate source execute per request. Each engine
uses4 MiB/64 KiB stack/100 interrupt polls; checkpoints allow128 jobs. The worker
process keeps its P1 16 MiB heap cap and180-tick total transport deadline. Outer
allocation failure may still terminate that worker; the controller stays alive.
These are fixture budgets, not modern-page working-set targets or conformance.

A session owns its child and is dropped after the reply. Exceptions retain earlier
completed effects; malformed replies, timeouts and cancellation discard the worker.
Navigation/close still cancels its shared token; late results cannot replace a new
page or address draft. The realm does not persist for user-input events: a proper
per-document task/event lifecycle is a next P3 dependency, not implemented here.

## Generic owned IPC for redirected user programs

The existing one-peer stream prevented a **redirected** application from attaching
its own worker. Forge currently launches jbrowse without redirection, so its
normal Terminal/desktop launch did not hit this limitation. The new nested-host
probe exercises the additional topology directly; it is not evidence that the old
default GUI launch failed to attach a worker. Syscall22 now accepts RDX=`owned_child_pid+1`
for send/receive; zero retains the original stream. A connected child may own one
separate descendant FIFO. SDK `Child` uses the selector; normal stdin/stdout still
reach the parent. A selector names only an already-owned endpoint, not an arbitrary
PID, parent, or foreign handle. Queue capacity remains4096 u32 entries per endpoint;
reservation is fallible and happens before either side is published.

Host exit/kill stops its entire owned chain. Cleanup uses constant stack space,
bounded128-slot traversals and leaf-first waiter wakeups, preserves already-exited
status, and reclaims stopped descendants after execution/TLB quiescence. Reaping a
worker releases only its owner's worker endpoint, leaving terminal I/O intact.
A connected process with a live owned descendant cannot enter the restricted-worker
profile. No new syscall number, user pointer, mapping or app policy was introduced.
See the [ABI contract](syscalls.md) for compatibility/readiness details.

## Executed evidence and remaining work

[Red/green logs and screenshots](browser-p3/README.md) distinguish host engine/DOM
checks from actual guest behavior. The old application failed to reach the native
promise fixture's title; the old worker rejected Document requests. The kernel
regression first failed because a connected app could not own a worker. During
cleanup implementation another regression caught an already-exited code23 being
overwritten with9; it was fixed without changing the assertion. Engine execution,
bounded jobs, host-call ownership, effect validation and native DOM tests all
started red before implementation.

The application now passes its local native fixture and prior browser cases.
Worker probes exercise both direct and nested service ownership, independent parent
stdin/stdout, foreign selector rejection, profile rejection, exact physical-frame
recovery, faults, budgets and malformed IPC on1/8/32 CPUs. Kernel units and stdio/
threads/display integrations were rerun for the changed ABI/lifecycle. All apps and
metadata were rebuilt. Host tooling, desktop and Google outcomes are recorded in
the evidence index. QMP input is not a manual host GUI/input-capture test.

Open P3 work: broader WPT/Web IDL bindings and Test262 measurement; persistent
per-document tasks, event dispatch/default actions/focus, timers/animation callbacks,
parser/defer/async/module ordering; URL/encoding/stream/form host primitives; real
Fetch/XHR methods/bodies and compression; cookies/storage/profiles; same-origin,
CORS/credentials/CSP/mixed-content/frame/secure-context policy; further common APIs.
Date/Intl and other deliberately absent engine profile capabilities also need
integration. No production sandbox, full renderer process split, Google results,
Facebook login/feed or YouTube playback is claimed. P4 remains gated on measured
P2/P3 compatibility, not merely on this engine running.

The [final native runs](browser-p3/README.md) passed the full browser and desktop
on8 CPUs, plus the direct/nested service on32 CPUs after refactoring channels into
[`scheduler::channels`](../src/scheduler/channels.rs). Earlier1/8/32-CPU native
service results are separately retained. The strict anonymous live Google search
check was rerun on2026-11-06 and **still failed**: homepage/submission reached the
response but results require browser APIs outside this profile. This remains the
user-visible goal, not a completed milestone. No manual host GUI test was run.

The final ready service added3354 physical frames in the1920×1080/1 GiB fixture,
including image, initial stack, display backing and heap/page-table overhead;
this supersedes the older P1 service measurement for this build. The probe also
bounds the live document footprint by the existing8192-frame envelope and requires
exact return to the pre-launch frame count. It is not a modern-page or total-memory
security quota. Evidence logs normalize trailing whitespace for repository checks.
