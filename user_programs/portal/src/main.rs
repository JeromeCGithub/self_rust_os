// Portal owns terminal presentation and input forwarding. Forge owns the shell.
mod screen;
use screen::{Screen, COLS};
use userlib::{
    println,
    process::Child,
    sys,
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
    let mut child = match Child::spawn("/BIN/FORGE.BIN") {
        Ok(child) => child,
        Err(()) => {
            println!("[portal] shell launch failed");
            sys::exit(1);
        }
    };
    println!("[portal] shell pid={}", child.pid());
    let mut screen = Screen::new();
    let mut pending = None;
    let mut dirty = true;
    let mut previous_blink = false;
    loop {
        for _ in 0..16 {
            if window.poll_event().expect("terminal events") == Some(Event::CloseRequested) {
                drop(child);
                sys::exit(0);
            }
        }
        // Drain a bounded output batch before forwarding more input. A batch
        // covers the endpoint capacity, then paints once for streamed responses.
        for _ in 0..4096 {
            match child.read() {
                Ok(Some((bytes, count))) => {
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
        let blink = (sys::ticks() / 9) % 2 == 0;
        dirty |= blink != previous_blink;
        if dirty {
            window
                .draw(|canvas| {
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
            dirty = false;
        }
        previous_blink = blink;
        sys::sleep_ticks(1).expect("terminal timed wait");
    }
}
