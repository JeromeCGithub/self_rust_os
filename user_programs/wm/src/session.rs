//! Compositor service: protocol dispatch, focus, stacking and lifecycle coordination.
use userlib::ui::{
    desktop_protocol::{self as shell, Managed, WorkArea},
    protocol::{self, Configuration},
};
use userlib::{println, sys};

use crate::damage::Damage;
use crate::display::{Compositor, MAX_H, MAX_PITCH, MAX_W, TITLE_H};
use crate::window::{free_slot, hit, new_window, Window, MAX};
fn focus(pid: u32) {
    let _ = sys::focus(pid);
}
pub(crate) fn run() {
    sys::graphics_mode();
    let info = match sys::fb_info() {
        Ok(i)
            if (640..=MAX_W).contains(&i.width)
                && (480..=MAX_H).contains(&i.height)
                && i.bpp == 32
                && i.pitch >= i.width * 4
                && i.pitch <= MAX_PITCH
                && i.pitch % 4 == 0 =>
        {
            i
        }
        _ => {
            println!("[desktop] unsupported display geometry");
            sys::exit(1)
        }
    };
    let mut display = Compositor {
        info,
        damage: Damage::new(),
        cursor: None,
        cursor_under: [0; 8 * 14],
    };
    display.clear_background();
    let mut shell_pid = match sys::spawn("/BIN/DE.BIN") {
        Ok(pid) => Some(pid),
        Err(()) => {
            println!("[compositor] desktop launch failed");
            None
        }
    };
    let mut shell_connected = false;
    let mut shell_commit = None;
    let mut shell_reply = None;
    let mut shell_release = None;
    let mut shell_pointer = None;
    let mut last_managed = None;
    let mut work = WorkArea { top: 0, bottom: 0 };
    let mut desktop_capture = false;
    println!("[compositor] ready");
    let mut windows: [Option<Window>; MAX] = [None; MAX];
    let mut used = 0;
    let mut previous = 0;
    let mut drag: Option<(i32, i32)> = None;
    let mut scene_dirty = true;
    let mut old_bounds = (0, 0, info.width, info.height);
    let mut focused = shell_pid.unwrap_or(0);
    focus(focused);
    let mut announced_focus = 0;
    display.damage.mark(0, 0, info.width, info.height);
    let mut old_mouse = None;
    loop {
        let mut revoke_desktop = false;
        // Opaque service messages are decoded here in Ring 3, never in the kernel.
        // Each iteration is bounded even if a malicious client floods its channel.
        for _ in 0..64 {
            let Ok(Some((pid, message))) = sys::receive_message() else {
                break;
            };
            let existing = windows[..used]
                .iter()
                .position(|window| window.map(|w| w.pid) == Some(pid));
            match message & protocol::KIND {
                shell::DESKTOP => {
                    if message == shell::DESKTOP
                        && shell::authorized(pid, shell_pid)
                        && !shell_connected
                    {
                        shell_connected = true;
                        shell_reply = Some(protocol::ACCEPT);
                    } else {
                        let _ = sys::send_message(pid, protocol::REJECT);
                    }
                }
                shell::WORK_AREA => {
                    if shell::authorized(pid, shell_pid) && shell_connected {
                        let area = WorkArea::decode(message, info.height).filter(|area| {
                            windows[..used].iter().flatten().all(|w| {
                                w.height
                                    + TITLE_H as usize
                                    + usize::from(area.top)
                                    + usize::from(area.bottom)
                                    <= info.height
                            })
                        });
                        if let Some(area) = area {
                            work = area;
                            for window in windows[..used].iter_mut().flatten() {
                                window.y = window.y.clamp(
                                    i32::from(work.top),
                                    info.height as i32
                                        - i32::from(work.bottom)
                                        - TITLE_H
                                        - window.height as i32,
                                );
                            }
                            scene_dirty = true;
                            shell_reply = Some(protocol::ACCEPT | shell::WORK_REPLY);
                        } else {
                            shell_reply = Some(protocol::REJECT | shell::WORK_REPLY);
                        }
                    } else {
                        let _ = sys::send_message(pid, protocol::REJECT);
                    }
                }
                shell::MANAGE => {
                    if !shell::authorized(pid, shell_pid) || !shell_connected {
                        let _ = sys::send_message(pid, protocol::REJECT);
                        continue;
                    }
                    let Some(request) = Managed::decode(message) else {
                        shell_reply = Some(protocol::REJECT | (message & shell::PID_MASK));
                        continue;
                    };
                    if Some(request.pid) == shell_pid || sys::surface_status(request.pid).is_err() {
                        shell_reply = Some(protocol::REJECT | request.pid);
                        continue;
                    }
                    let index = windows[..used]
                        .iter()
                        .position(|w| w.map(|w| w.pid) == Some(request.pid));
                    let index = if let Some(index) = index {
                        index
                    } else {
                        if used == MAX {
                            shell_reply = Some(protocol::REJECT | request.pid);
                            continue;
                        }
                        let slot = free_slot(&windows[..used]);
                        windows[used] = Some(new_window(request.pid, slot, used));
                        used += 1;
                        used - 1
                    };
                    let window = windows[index].as_mut().expect("managed window");
                    if !window.native {
                        window.large = request.large;
                    }
                    last_managed = Some(request.pid);
                    shell_reply = Some(protocol::ACCEPT | request.pid);
                    focused = request.pid;
                    focus(focused);
                    scene_dirty = true;
                }
                shell::MANAGED_TITLE => {
                    if shell::authorized(pid, shell_pid) && shell_connected {
                        if let Some(window) = windows[..used]
                            .iter_mut()
                            .flatten()
                            .find(|w| Some(w.pid) == last_managed && !w.native)
                        {
                            let chunk = ((message >> 24) & 15) as usize;
                            if chunk < 8 {
                                for byte in 0..3 {
                                    let value = ((message >> (byte * 8)) & 255) as u8;
                                    window.title[chunk * 3 + byte] =
                                        if value == 0 || (32..127).contains(&value) {
                                            value
                                        } else {
                                            b'?'
                                        };
                                }
                                scene_dirty = true;
                            }
                        }
                    } else {
                        let _ = sys::send_message(pid, protocol::REJECT);
                    }
                }
                protocol::CONFIGURE => {
                    let config = Configuration::decode(message).filter(|c| {
                        c.width <= info.width
                            && c.height
                                + TITLE_H as usize
                                + usize::from(work.top)
                                + usize::from(work.bottom)
                                <= info.height
                    });
                    if Some(pid) == shell_pid
                        || config.is_none()
                        || sys::surface_status(pid)
                            .map(|(_, dead)| dead)
                            .unwrap_or(true)
                        || existing
                            .map(|i| windows[i].unwrap().native)
                            .unwrap_or(false)
                    {
                        let _ = sys::send_message(pid, protocol::REJECT);
                        continue;
                    }
                    let index = if let Some(index) = existing {
                        index
                    } else {
                        if used == MAX {
                            let _ = sys::send_message(pid, protocol::REJECT);
                            continue;
                        }
                        let slot = free_slot(&windows[..used]);
                        windows[used] = Some(new_window(pid, slot, used));
                        used += 1;
                        focused = pid;
                        focus(pid);
                        used - 1
                    };
                    let config = config.expect("validated configuration");
                    let window = windows[index].as_mut().expect("registered window");
                    window.width = config.width;
                    window.height = config.height;
                    window.native = true;
                    window.material = config.material;
                    window.reply = Some(protocol::ACCEPT);
                    window.focus_event = Some(window.pid == focused);
                    window.x = window.x.clamp(0, info.width as i32 - window.width as i32);
                    window.y = window.y.clamp(
                        i32::from(work.top),
                        info.height as i32
                            - i32::from(work.bottom)
                            - TITLE_H
                            - window.height as i32,
                    );
                    display.reset_content(window);
                    scene_dirty = true;
                }
                protocol::TITLE => {
                    if let Some(index) = existing {
                        let window = windows[index].as_mut().expect("registered window");
                        let chunk = ((message >> 24) & 15) as usize;
                        if window.native && chunk < 8 {
                            for byte in 0..3 {
                                let value = ((message >> (byte * 8)) & 255) as u8;
                                window.title[chunk * 3 + byte] =
                                    if value == 0 || (32..127).contains(&value) {
                                        value
                                    } else {
                                        b'?'
                                    };
                            }
                            scene_dirty = true;
                        }
                    }
                }
                protocol::COMMIT => {
                    if shell::authorized(pid, shell_pid) && shell_connected {
                        if shell_commit.is_none() {
                            shell_commit = Some(message & protocol::SERIAL);
                        }
                        continue;
                    }
                    if let Some(index) = existing {
                        let window = windows[index].as_mut().expect("registered window");
                        if window.native && window.commit.is_none() {
                            window.commit = Some(message & protocol::SERIAL);
                        }
                    }
                }
                protocol::CLOSE => {
                    if shell::authorized(pid, shell_pid) && shell_connected {
                        revoke_desktop = true;
                        continue;
                    }
                    if let Some(index) = existing {
                        let window = windows[index].as_mut().expect("registered window");
                        if window.native {
                            window.closing = Some(sys::ticks());
                        }
                    }
                }
                _ => {}
            }
        }
        if revoke_desktop {
            shell_connected = false;
            shell_commit = None;
            shell_release = None;
            shell_pointer = None;
            work = WorkArea { top: 0, bottom: 0 };
            display.clear_background();
            old_bounds = (0, 0, info.width, info.height);
            scene_dirty = true;
            if Some(focused) == shell_pid {
                focused = windows[..used]
                    .iter()
                    .rev()
                    .flatten()
                    .next()
                    .map(|w| w.pid)
                    .unwrap_or(0);
                focus(focused);
            }
        }
        // Native exit/close frees the WM cache slot; legacy text apps retain their
        // final output until closed. Existing kernel frame/stack leaks are separate.
        let mut index = 0;
        while index < used {
            let window = windows[index].expect("registered window");
            let dead = sys::surface_status(window.pid)
                .map(|(_, dead)| dead)
                .unwrap_or(true);
            if window.native
                && (dead
                    || window
                        .closing
                        .map(|tick| sys::ticks().wrapping_sub(tick) >= 18)
                        .unwrap_or(false))
            {
                if !dead {
                    let _ = sys::kill(window.pid);
                }
                let _ = sys::waitpid(window.pid);
                for j in index..used - 1 {
                    windows[j] = windows[j + 1];
                }
                used -= 1;
                windows[used] = None;
                if focused == window.pid {
                    focused = if used > 0 {
                        windows[used - 1].unwrap().pid
                    } else {
                        shell_pid.unwrap_or(0)
                    };
                    focus(focused);
                }
                drag = None;
                scene_dirty = true;
            } else {
                index += 1;
            }
        }
        let (mx, my, buttons) = sys::mouse();
        let down = buttons & 1 != 0;
        let click = down && previous & 1 == 0;
        if click {
            let window_hit = (0..used).rev().find(|&i| {
                let w = windows[i].unwrap();
                hit(mx, my, w.x, w.y, w.width as i32, w.height as i32 + TITLE_H)
            });
            desktop_capture = window_hit.is_none();
            if let Some(i) = window_hit {
                let w = windows[i].unwrap();
                // Clicking the focused top window changes client input only.
                // Recomposing the unchanged scene here delays its pressed frame;
                // normal surface revision damage covers the client update.
                scene_dirty |= i + 1 != used || focused != w.pid;
                for j in i..used - 1 {
                    windows[j] = windows[j + 1];
                }
                windows[used - 1] = Some(w);
                focused = w.pid;
                focus(w.pid);
                if hit(mx, my, w.x + w.width as i32 - 33, w.y, 33, TITLE_H) {
                    if w.native && !w.exited {
                        let _ = sys::send_message(w.pid, protocol::CLOSE);
                        windows[used - 1].as_mut().unwrap().closing = Some(sys::ticks());
                    } else {
                        let _ = sys::kill(w.pid);
                        let _ = sys::waitpid(w.pid);
                        used -= 1;
                        windows[used] = None;
                        scene_dirty = true;
                        focused = if used > 0 {
                            windows[used - 1].unwrap().pid
                        } else {
                            shell_pid.unwrap_or(0)
                        };
                        focus(focused);
                    }
                } else if hit(mx, my, w.x, w.y, w.width as i32 - 33, TITLE_H) {
                    drag = Some((mx - w.x, my - w.y));
                    println!("[desktop] drag started: pid={}", w.pid);
                }
            } else {
                focused = shell_pid.unwrap_or(0);
                focus(focused);
                scene_dirty = true;
            }
        }
        if !down {
            drag = None;
        }
        if let Some((dx, dy)) = drag {
            if used > 0 {
                let w = windows[used - 1].as_mut().unwrap();
                let x = (mx - dx).clamp(0, info.width as i32 - w.width as i32);
                let y = (my - dy).clamp(
                    i32::from(work.top),
                    info.height as i32 - i32::from(work.bottom) - TITLE_H - w.height as i32,
                );
                scene_dirty |= (w.x, w.y) != (x, y);
                (w.x, w.y) = (x, y);
            }
        }
        if focused != announced_focus {
            for window in windows[..used].iter_mut().flatten().filter(|w| w.native) {
                window.focus_event = Some(window.pid == focused);
                window.pointer = None;
            }
            announced_focus = focused;
            scene_dirty = true;
        }
        let mut compose = scene_dirty;
        for window in windows[..used].iter_mut().flatten() {
            compose |= display.update_surface(window);
            // Frame/config acknowledgements have priority over coalesced motion.
            if let Some(reply) = window.reply {
                if sys::send_message(window.pid, reply).is_ok() {
                    window.reply = None;
                }
            }
            if let Some(focused) = window.focus_event {
                if sys::send_message(window.pid, protocol::FOCUS | u32::from(focused)).is_ok() {
                    window.focus_event = None;
                }
            }
            if window.native && window.pid == focused && drag.is_none() {
                let pointer = (
                    (mx - window.x).clamp(-1024, 1023),
                    (my - window.y - TITLE_H).clamp(-1024, 1023),
                    buttons,
                );
                if window.pointer != Some(pointer) {
                    let payload = protocol::POINTER
                        | (pointer.0 as u32 & 2047)
                        | ((pointer.1 as u32 & 2047) << 11)
                        | (u32::from(buttons & 7) << 22);
                    if sys::send_message(window.pid, payload).is_ok() {
                        window.pointer = Some(pointer);
                    }
                }
            }
        }
        if let Some(pid) = shell_pid {
            if sys::surface_status(pid)
                .map(|(_, dead)| dead)
                .unwrap_or(true)
            {
                let _ = sys::waitpid(pid);
                shell_pid = None;
                shell_connected = false;
                shell_pointer = None;
                shell_commit = None;
                shell_reply = None;
                shell_release = None;
                work = WorkArea { top: 0, bottom: 0 };
                display.clear_background();
                old_bounds = (0, 0, info.width, info.height);
                compose = true;
                println!("[compositor] desktop disconnected");
                if focused == pid {
                    focused = windows[..used]
                        .iter()
                        .rev()
                        .flatten()
                        .next()
                        .map(|w| w.pid)
                        .unwrap_or(0);
                    focus(focused);
                }
            } else {
                if let Some(serial) = shell_commit.take() {
                    if display.snapshot_desktop(pid) {
                        shell_release = Some(protocol::RELEASE | serial);
                        compose = true;
                        old_bounds = (0, 0, info.width, info.height);
                        display.damage.mark(0, 0, info.width, info.height);
                        println!("[compositor] desktop surface committed");
                    } else {
                        shell_reply = Some(protocol::REJECT);
                    }
                }
                if let Some(reply) = shell_reply {
                    if sys::send_message(pid, reply).is_ok() {
                        shell_reply = None;
                    }
                }
                if let Some(reply) = shell_release {
                    if sys::send_message(pid, reply).is_ok() {
                        shell_release = None;
                    }
                }
                let over_window = windows[..used]
                    .iter()
                    .flatten()
                    .any(|w| hit(mx, my, w.x, w.y, w.width as i32, w.height as i32 + TITLE_H));
                if shell_connected && (!over_window || desktop_capture) && drag.is_none() {
                    let buttons = if desktop_capture {
                        buttons
                    } else {
                        buttons & !1
                    };
                    let payload = shell::pointer(mx as u16, my as u16, buttons);
                    if shell_pointer != Some(payload) && sys::send_message(pid, payload).is_ok() {
                        shell_pointer = Some(payload);
                    }
                }
            }
        }
        if !down {
            desktop_capture = false;
        }
        if compose {
            let mut bounds = (info.width, info.height, 0, 0);
            for window in windows[..used].iter().flatten() {
                bounds.0 = bounds.0.min((window.x - 12).max(0) as usize);
                bounds.1 = bounds.1.min((window.y - 8).max(0) as usize);
                bounds.2 = bounds
                    .2
                    .max((window.x as usize + window.width + 12).min(info.width));
                bounds.3 = bounds.3.max(
                    (window.y as usize + window.height + TITLE_H as usize + 16).min(info.height),
                );
            }
            // Restore and compose bottom-to-top on *any* content change. Glass
            // depends on lower layers even if its own client's revision is stable.
            // This also prevents repeatedly blurring the previous glass result.
            display.restore_background((
                old_bounds.0.min(bounds.0),
                old_bounds.1.min(bounds.1),
                old_bounds.2.max(bounds.2),
                old_bounds.3.max(bounds.3),
            ));
            for window in windows[..used].iter().flatten() {
                display.window(window, window.pid == focused);
            }
            old_bounds = bounds;
            scene_dirty = false;
        }
        let moved = old_mouse != Some((mx, my));
        if moved {
            if let Some((x, y)) = old_mouse {
                display.damage.mark(x as usize, y as usize, 8, 14);
            }
            display.damage.mark(mx as usize, my as usize, 8, 14);
        }
        display.present(mx, my);
        old_mouse = Some((mx, my));
        previous = buttons;
        sys::sleep_ticks(1).expect("desktop timed wait");
    }
}
