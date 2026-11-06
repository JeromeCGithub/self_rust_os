# jbrowse inline JavaScript prototype

Historical prototype profile: the application now uses [native QuickJS/DOM](browser-javascript-p3.md).
This document describes the optional old library feature and its original evidence.

This branch extends [the HTML/CSS browser](jbrowse.md) with a small bounded
straight-line JavaScript subset. It is **not** an ECMAScript engine or support for
modern JavaScript applications. Use the HTML/CSS branch independently if scripting
is not wanted.

## Try a scripted page

Serve the checked-in [example](../user_programs/jbrowse/examples/scripted.html)
from the host:

```bash
python3 -m http.server 8080 --bind 127.0.0.1 --directory user_programs/jbrowse/examples
```

Open `http://10.0.2.2:8080/scripted.html` in jbrowse under QEMU's user network.
The paragraph changes to `Answer: 42`, the document title changes, and the
background becomes lavender. [The QEMU screenshot](screenshots/jbrowse-scripted.png)
shows the equivalent automated fixture with asserted text and background colors.

[CERN's original World Wide Web site](http://info.cern.ch/hypertext/WWW/TheProject.html)
remains the real public HTML navigation test. Its success does not demonstrate
JavaScript support. Script behavior is verified with the independent local HTTP
fixture, rather than claiming compatibility with an untested public JS site.

## Supported language and DOM surface

Inline classic scripts run once in source order, after HTML and linked CSS have
been fetched and parsed, before layout. Variables persist between script elements
within that load; navigation gets a fresh runtime. Scripts are not parser-blocking.
Empty/classic JavaScript types are recognized; JSON and other types are ignored.
External scripts report unsupported; modules are not executed.

The evaluator supports initialized `let`, `const` and `var` bindings, variable
assignment, finite decimal numbers, strings, booleans, null, parentheses, unary
plus/minus, arithmetic `+ - * /`, string concatenation, comments, and explicit
semicolon-separated statements. String escapes cover newline, carriage return,
tab, quote and backslash. Number coercion and grammar are deliberately incomplete.
Non-finite results are rejected.

Typed DOM capabilities expose:

- Read/write `document.title`.
- Read/write `document.getElementById('id').textContent` and
  `document.body.textContent`. Writes insert literal text and detach children;
  text never gets reparsed as HTML. Lookup visits connected nodes only.
- Write `.style.color`, `.style.backgroundColor` and `.style.display`
  on those elements. Colors and display values are validated/normalized before
  entering CSS declarations; strings cannot inject additional declarations.

Unsupported features include functions, loops, conditions, objects/arrays,
events/handlers, timers, promises, modules, external scripts, browser storage,
cookies, fetch/XHR, navigation from scripts, eval, and arbitrary DOM construction.
This does not run React, Google web apps or JavaScript-heavy sites. It provides an
initial DOM/evaluator seam while fuller engine integration is investigated.

## Architecture and bounds

`crates/script` is `no_std + alloc`, with no OS or browser dependency.
Its `Host` trait accepts only typed property reads/writes. It has no syscall,
file, socket, clock or random capability. `crates/browser/src/script.rs` adapts
those calls to the document arena; the browser crate's `javascript` feature is
optional and disabled by default. The jbrowse executable enables it explicitly.

Scripts run in the application's existing resource worker. No kernel code,
syscall numbers, userlib or compositor protocol changed in this stack layer.
The main thread continues handling window events during loading/evaluation.

Per document: 32 KiB aggregate source, 8192 aggregate tokens, 4096 tokens per
script, 32 expression nesting levels, 128 variables, and 8192-byte string/DOM
values. The DOM retains its HTML/node budgets. Detached subtrees remain arena
storage until navigation drops the document; script cannot allocate new nodes.
Style strings also have an 8192-byte cap.

Unsupported syntax, missing names/elements, readonly assignment, type errors and
budgets return explicit errors. An error stops that script, preserving completed
effects; later script elements can continue. The browser shows a script error
count while retaining readable HTML. There is no JIT, native code generation,
new unsafe code, or claim of production web isolation.

## Engine research and next step

Checked on 2026-11-05 against primary project sources:

| Candidate | What was established | Porting implication |
| --- | --- | --- |
| [Boa manifest](https://github.com/boa-dev/boa/blob/main/core/engine/Cargo.toml) | Engine dependencies explicitly enable std in several libraries and use a substantially larger runtime | It is not a drop-in dependency for this pinned no_std target; a real port needs its own toolchain/dependency investigation |
| [QuickJS embedding guide](https://bellard.org/quickjs/quickjs.html) | C embedding API, runtime memory limit and interrupt hooks are documented | A userland C/libc and math adapter, reproducible build integration, allocator/exception ownership and DOM bindings need implementation/testing |
| [MicroQuickJS](https://github.com/bellard/mquickjs/blob/main/README.md) | Designed for constrained systems, uses a caller-supplied memory block, exposes a smaller JavaScript subset | Promising next candidate for a bounded Ring 3 engine port; it still requires a C toolchain/runtime integration and tested host bindings |

The inference from those sources is that fuller engines deserve a separate port
rather than incidental kernel/libc/toolchain changes in this browser PR. None of
those engines is vendored or claimed to run here. The current evaluator is an
explicit prototype, replaceable behind the typed DOM host boundary. A fuller
port should add bounded execution interrupts, allocation recovery, parser/VM
conformance tests and real guest execution before extending browser claims.

## Executed validation and reproduction

Red: all four initial DOM scripting tests failed with no script execution.
The QEMU scripting run against the HTML/CSS-only executable loaded
`Before script` and failed waiting for `Script ready`.

Host commands (run Cargo from `/tmp`, outside the bare-metal root config):

```bash
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/script/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/browser/Cargo.toml --target x86_64-unknown-linux-gnu
cargo +nightly-2025-01-30 test --manifest-path /path/to/jbrowse/crates/browser/Cargo.toml --target x86_64-unknown-linux-gnu --features javascript
```

The interpreter's three cases exercise precedence/unary/decimal arithmetic,
comments/getters/string escapes, independent source/token/nesting/variable/string
limits and continued use after errors. Four DOM cases cover text/title/style
mutation, source-order realm sharing, fresh navigation state, unsupported loops,
missing elements, readonly values, budgets and declaration/markup injection.
All seven existing HTML/CSS tests also run with the feature enabled.

```bash
./tools/build-grub-disk.sh target/jbrowse-js.img
RUSTOS_TEST_DISK=target/jbrowse-js.img RUSTOS_JBROWSE_SCRIPTS=1 RUSTOS_JBROWSE_PUBLIC=1 python3 -B e2e/jbrowse.py
RUSTOS_TEST_DISK=target/jbrowse-js.img python3 -B e2e/desktop.py
python3 -B tools/check_docs.py
```

QEMU asserts a script-generated document title, lavender background, text color,
unsupported-loop recovery and subsequent navigation, in addition to the parent
browser acceptance cases and the real CERN request. PR evidence distinguishes
these checks from inherited HTTP/TLS/UI results. Manual host GUI testing remains
outstanding. No modern website or full JavaScript conformance has been verified.

The browser now opens Google by default and has generic GET form controls; see
[the Google status guide](jbrowse-google.md). Google search results still require
JavaScript beyond this prototype. A script-only blank page now gets an explicit
unsupported-script fallback, covered by QEMU pixel assertions.
