# Pinned native JavaScript feasibility dependencies

`upstream/` is the selected, **unmodified** source subset from the official
[QuickJS 2026-06-04 release](https://bellard.org/quickjs/quickjs-2026-06-04.tar.xz).
The archive SHA-256 and each selected file's digest are in [VENDOR.json](VENDOR.json).
`nanoprintf.h` is pinned to commit
`bebaecbeaf430e36a2d30d31a2fed4e098388c93` of
[nanoprintf](https://github.com/charlesnicholson/nanoprintf).
QuickJS is MIT licensed; this use of nanoprintf selects its 0BSD option (text at
the end of its header). Keep copyright/license notices with redistributed sources.
Git attributes mark upstream code as vendored and preserve its existing trailing
spaces; local adapter code retains ordinary whitespace checks.
The Rust math dependency is separately pinned to libm 0.2.16 (MIT in its manifest).

Run `python3 -B tools/check_quickjs_vendor.py third_party/quickjs` from the repository.
Every engine build also checks those digests. Updates require replacing sources,
reviewing release/security changes and licenses, regenerating the manifest from
the downloaded inputs, auditing native undefined symbols and guarded build
configuration edits, then rerunning both host profiles and the Ring 3 probe.
A digest detects accidental source drift; it is not upstream signature verification.

The [build adapter](../../crates/quickjs/build.rs) creates a configured copy of
`quickjs.c` in Cargo's output directory. Two guarded edits disable blocking Atomics
and allow the private allocator symbol remapping before QuickJS's allocator guard.
Date is excluded by context construction. `quickjs-libc.c`, the shell, bytecode
loader, module loader, OS/process/file helpers and upstream build artifacts are
not included. No page-provided bytecode is loaded.

See [P0 architecture and evidence](../../docs/browser-engine-p0.md) for scope,
platform assumptions, native results, missing capabilities and the update gate.
