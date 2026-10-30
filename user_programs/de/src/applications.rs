//! Application discovery and legacy launch compatibility policy.
use userlib::sys;
pub(super) const MAX: usize = 6;
#[derive(Clone, Copy)]
pub(super) struct App {
    pub(super) name: [u8; 12],
    pub(super) path: [u8; 32],
}
pub(super) const EMPTY_APP: App = App {
    name: [0; 12],
    path: [0; 32],
};
pub(super) fn string(bytes: &[u8]) -> &str {
    core::str::from_utf8(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .unwrap_or("")
}
pub(super) fn discover(apps: &mut [App; MAX]) -> usize {
    let mut entries = [0u8; 12 * 32];
    let mut count = 0;
    if let Some(n) = sys::ls("/BIN", &mut entries, 32) {
        for e in entries.chunks_exact(12).take(n.min(32)) {
            if &e[8..11] != b"BIN"
                || (&e[..8] == b"WM      " || &e[..8] == b"DE      ")
                || count == MAX
            {
                continue;
            }
            let len = e[..8].iter().position(|b| *b == b' ').unwrap_or(8);
            let mut app = EMPTY_APP;
            app.name[..len].copy_from_slice(&e[..len]);
            if &e[..len] == b"PORTAL" {
                app.name = [0; 12];
                app.name[..8].copy_from_slice(b"Terminal");
            }
            app.path[..5].copy_from_slice(b"/BIN/");
            app.path[5..5 + len].copy_from_slice(&e[..len]);
            app.path[5 + len..9 + len].copy_from_slice(b".BIN");
            apps[count] = app;
            count += 1;
        }
    }
    count
}

impl App {
    /// Spawn and register a window; reap the child if registration fails.
    pub(super) fn launch(self, surface: &mut userlib::ui::DesktopSurface) {
        let app = self;
        match sys::spawn(string(&app.path)) {
            Ok(pid) => {
                // Executable-specific compatibility policy stays
                // in the shell, outside the compositor.
                let large = string(&app.name) == "SNAKE";
                if surface.manage(pid, string(&app.name), large).is_ok() {
                    userlib::println!("[desktop] launched {} pid={}", string(&app.name), pid);
                } else {
                    let _ = sys::kill(pid);
                    let _ = sys::waitpid(pid);
                    userlib::println!("[desktop] window request rejected");
                }
            }
            Err(()) => {
                userlib::println!("[desktop] launch failed: {}", string(&app.path))
            }
        }
    }
}
