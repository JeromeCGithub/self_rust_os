// Portal owns terminal presentation and input forwarding. Forge owns the shell.
mod screen;
use core::sync::atomic::{AtomicU32, Ordering};
use screen::{Screen, COLS};
use userlib::{
    println,
    process::Child,
    sync::{Arc, Mutex},
    sys, thread,
    ui::{self, Color, Event, Material, Rect, Window, WindowOptions},
};

fn main() {
    println!("[portal] Portal starting...");
    let mut window = Window::create(
        WindowOptions::new("Terminal")
            .content_size(488, 288)
            .background(Material::Glass),
    )
    .expect("create terminal window");
    let shared = Arc::new(Shared {
        screen: Mutex::new(Screen::new()),
        revision: AtomicU32::new(1),
    });
    let transport = Arc::clone(&shared);
    let _io = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || run_io(transport))
        .expect("terminal I/O thread");
    let mut painted = 0;
    let mut previous_blink = false;
    loop {
        for _ in 0..16 {
            if window.poll_event().expect("terminal events") == Some(Event::CloseRequested) {
                // Process exit stops the I/O thread and its owned shell together.
                sys::exit(0);
            }
        }
        let revision = shared.revision.load(Ordering::Acquire);
        let blink = (sys::ticks() / 9) % 2 == 0;
        if revision != painted || blink != previous_blink {
            window
                .draw(|canvas| {
                    // draw invokes this only after the preceding frame ACK.
                    // Coalesce all text that arrived during that wait, instead
                    // of submitting an already stale snapshot. The revision is
                    // acquired before cloning; a concurrent newer update will
                    // still differ from `painted` and request another frame.
                    painted = shared.revision.load(Ordering::Acquire);
                    let screen = shared.screen.lock().clone();
                    for (i, line) in screen.lines.iter().enumerate() {
                        let len = line.iter().position(|b| *b == 0).unwrap_or(COLS);
                        ui::text(
                            canvas,
                            12,
                            12 + i as i32 * 12,
                            core::str::from_utf8(&line[..len]).unwrap_or(""),
                            Color::rgb(0xbed5e8),
                            1,
                        );
                    }
                    if blink {
                        canvas.fill_rect(
                            Rect::new(
                                12 + (screen.column.min(COLS - 1) * 8) as i32,
                                12 + screen.row as i32 * 12,
                                8,
                                12,
                            ),
                            Color::rgb(0xffffff),
                        );
                    }
                })
                .expect("terminal frame");
        }
        previous_blink = blink;
        // Text updates wake immediately. The bounded deadline services window
        // messages and cursor animation even when the shell produces no output.
        thread::wait(&shared.revision, painted, Some(1)).expect("terminal render wait");
    }
}

struct Shared {
    screen: Mutex<Screen>,
    revision: AtomicU32,
}

// Input/output must keep progressing while the renderer waits for the WM.
fn run_io(shared: Arc<Shared>) {
    let mut child = match Child::spawn("/BIN/FORGE.BIN") {
        Ok(child) => child,
        Err(()) => {
            println!("[portal] shell launch failed");
            sys::exit(1);
        }
    };
    println!("[portal] shell pid={}", child.pid());
    let mut pending = None;
    loop {
        let mut dirty = false;
        // Drain a bounded output batch before forwarding more input. A batch
        // covers the endpoint capacity, then paints once for streamed responses.
        for _ in 0..4096 {
            match child.read() {
                Ok(Some((bytes, count))) => {
                    let mut screen = shared.screen.lock();
                    for &byte in &bytes[..count] {
                        if byte == b'\n' {
                            println!(
                                "[portal] output: {}",
                                core::str::from_utf8(screen.current()).unwrap_or("?")
                            );
                        }
                        screen.put(byte);
                    }
                    dirty = true;
                }
                Ok(None) => break,
                Err(_) => {
                    println!("[portal] shell disconnected");
                    drop(child);
                    sys::exit(0);
                }
            }
        }
        for _ in 0..64 {
            if pending.is_none() {
                let mut byte = [0];
                if sys::read_nb(sys::STDIN_FD, &mut byte) == 0 {
                    break;
                }
                pending = Some(byte[0]);
            }
            match child.write_byte(pending.expect("pending input")) {
                Ok(()) => pending = None,
                Err(sys::MessageError::Full) => break,
                Err(_) => {
                    drop(child);
                    sys::exit(0);
                }
            }
        }
        if dirty {
            // Publish the completed text update before waking the renderer.
            shared.revision.fetch_add(1, Ordering::Release);
            thread::wake(&shared.revision, 1).expect("wake terminal renderer");
        }
        let events = sys::Events::STDIO_READ
            | if pending.is_some() {
                sys::Events::STDIO_WRITE
            } else {
                sys::Events::KEYBOARD
            };
        sys::wait_events(events, 0).expect("terminal I/O readiness");
    }
}
