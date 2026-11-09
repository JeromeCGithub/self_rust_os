use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "initator-host-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn executable_loads_a_real_catalog_and_reports_actual_host_readiness() {
    let directory = Directory::new();
    let service=format!("Version=1\nKind=service\nName=beacon\nExec={}\nArg=notify\nReady=notify\nStartTimeoutTicks=100\n",env!("CARGO_BIN_EXE_service-fixture"));
    fs::write(directory.0.join("A.UNIT"), service).unwrap();
    fs::write(
        directory.0.join("BOOT.UNIT"),
        "Version=1\nKind=target\nName=boot\nRequires=beacon\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_initator-host"))
        .arg(&directory.0)
        .arg("500")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Started") && text.contains("Ready"), "{text}");
}
#[test]
fn executable_rejects_an_invalid_catalog_before_launching_anything() {
    let directory = Directory::new();
    fs::write(directory.0.join("BROKEN.UNIT"), "Version=1\nBogus=1\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_initator-host"))
        .arg(&directory.0)
        .arg("500")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("configuration"));
    assert!(output.stdout.is_empty());
}
