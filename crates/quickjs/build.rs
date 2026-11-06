use std::{env, fs, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let status = command.status().expect("start engine build command");
    assert!(status.success(), "engine build command failed: {command:?}");
}
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let vendor = root
        .join("../../third_party/quickjs")
        .canonicalize()
        .expect("vendor directory");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let target = env::var("TARGET").expect("target");
    let host = env::var("HOST").expect("host");
    assert!(
        host == "x86_64-unknown-linux-gnu",
        "P0 compiler adapter requires an x86-64 Linux host"
    );
    assert!(
        target == host || target == "x86_64-user-program",
        "unsupported P0 engine target: {target}"
    );
    let native = target != host || env::var_os("CARGO_FEATURE_FREESTANDING").is_some();
    run(Command::new("python3")
        .arg(root.join("../../tools/check_quickjs_vendor.py"))
        .arg(&vendor));
    println!("cargo:rerun-if-changed={}", vendor.display());
    println!("cargo:rerun-if-changed=c");
    println!("cargo:rerun-if-changed=../../tools/check_quickjs_vendor.py");
    println!("cargo:rerun-if-env-changed=CC");
    // Keep the vendored release byte-for-byte intact. Apply these documented
    // configuration patches in OUT_DIR, with an exact-match guard against drift.
    let source = fs::read_to_string(vendor.join("upstream/quickjs.c")).expect("QuickJS source");
    let atomics = "#if !defined(__EMSCRIPTEN__)\n#define CONFIG_ATOMICS\n#endif";
    assert_eq!(
        source.matches(atomics).count(),
        1,
        "Atomics configuration patch drift"
    );
    let mut configured = source.replace(
        atomics,
        "/* RustOS P0: no blocking Atomics host capability. */",
    );
    let forbidden = "#define malloc(s) malloc_is_forbidden(s)";
    assert_eq!(
        configured.matches(forbidden).count(),
        1,
        "allocator guard patch drift"
    );
    configured = configured.replace(
        forbidden,
        "#undef malloc\n#undef free\n#undef realloc\n#define malloc(s) malloc_is_forbidden(s)",
    );
    fs::write(out.join("quickjs.c"), configured).expect("configured engine source");
    let cc = env::var("CC").unwrap_or_else(|_| "gcc".into());
    let mut sources = vec![out.join("quickjs.c")];
    for name in ["dtoa.c", "libregexp.c", "libunicode.c", "cutils.c"] {
        sources.push(vendor.join("upstream").join(name));
    }
    sources.push(root.join("c/bridge.c"));
    if native {
        sources.push(root.join("c/platform.c"));
    }
    let mut objects = Vec::new();
    for (index, source) in sources.iter().enumerate() {
        let object = out.join(format!("engine-{index}.o"));
        let mut command = Command::new(&cc);
        command
            .args([
                "-c",
                "-std=gnu11",
                "-O2",
                "-fPIC",
                "-fno-stack-protector",
                "-fno-builtin",
                "-ffunction-sections",
                "-fdata-sections",
                "-mno-red-zone",
                "-mno-avx",
                "-msse2",
                "-mfpmath=sse",
                "-D_GNU_SOURCE",
                "-DCONFIG_VERSION=\"2026-06-04\"",
            ])
            .arg("-I")
            .arg(vendor.join("upstream"))
            .arg("-I")
            .arg(&vendor)
            .arg("-I")
            .arg(root.join("c"));
        if native {
            command
                .arg("-ffreestanding")
                .arg("-include")
                .arg(root.join("c/platform.h"));
        }
        command.arg(source).arg("-o").arg(&object);
        run(&mut command);
        objects.push(object);
    }
    let archive = out.join("librustos_quickjs_c.a");
    // Build into a fresh archive, never retain stale members after source changes.
    if archive.exists() {
        fs::remove_file(&archive).expect("replace engine archive");
    }
    run(Command::new("ar").arg("crs").arg(&archive).args(objects));
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=rustos_quickjs_c");
    if !native {
        println!("cargo:rustc-link-lib=m");
    }
}
