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
        pointer_input: crate::input::PointerInput::new(),
        caches: core::array::from_fn(|_| crate::cache::Cache::empty()),
        staging: core::array::from_fn(|_| None),
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
    let mut decoration_capture = false;
    println!("[compositor] ready");
    let mut windows: [Option<Window>; MAX] = [None; MAX];
    let mut used = 0;
    let mut previous = 0;
    let mut drag: Option<(i32, i32)> = None;
    let mut resize: Option<(u32, crate::geometry::Bounds, u8, i32, i32)> = None;
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
                                crate::geometry::accepts_reservation(
                                    crate::geometry::Workspace {
                                        width: info.width,
                                        height: info.height,
                                        top: usize::from(area.top),
                                        bottom: usize::from(area.bottom),
                                    },
                                    w.height,
                                    w.offered.is_some() || w.requested.is_some(),
                                )
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
                    let config = config.expect("validated configuration");
                    if existing.is_none() && used == MAX {
                        let _ = sys::send_message(pid, protocol::REJECT);
                        continue;
                    }
                    let slot = existing
                        .map(|i| windows[i].unwrap().cache_slot)
                        .unwrap_or_else(|| free_slot(&windows[..used]));
                    // Reserve the complete cache before publishing a new window,
                    // changing focus or replacing any client geometry.
                    if !display.allocate_content(slot, config.width, config.height) {
                        if let Some(index) = existing {
                            windows[index].as_mut().unwrap().reply = Some(protocol::REJECT);
                        } else {
                            let _ = sys::send_message(pid, protocol::REJECT);
                        }
                        continue;
                    }
                    let index = if let Some(index) = existing {
                        index
                    } else {
                        windows[used] = Some(new_window(pid, slot, used));
                        used += 1;
                        focused = pid;
                        focus(pid);
                        used - 1
                    };
                    let window = windows[index].as_mut().expect("registered window");
                    window.width = config.width;
                    window.height = config.height;
                    window.resizable = message & protocol::VERSION2 != 0;
                    window.minimum = (config.width.min(320), config.height.min(200));
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
                resize = None;
                display.release_cache(window.cache_slot);
                scene_dirty = true;
            } else {
                index += 1;
            }
        }
        let (mx, my, buttons) = display.next_pointer();
        let down = buttons & 1 != 0;
        let click = down && previous & 1 == 0;
        if click {
            decoration_capture = false;
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
                let edges = if w.resizable
                    && !w.expansion.expanded()
                    && w.offered.is_none()
                    && w.requested.is_none()
                {
                    crate::geometry::edges(w.bounds(), mx, my)
                } else {
                    0
                };
                decoration_capture = edges != 0 || my < w.y + TITLE_H;
                if edges != 0 {
                    resize = Some((w.pid, w.bounds(), edges, mx, my));
                } else if hit(mx, my, w.x + w.width as i32 - 33, w.y, 33, TITLE_H) {
                    if w.native && !w.exited {
                        let _ = sys::send_message(w.pid, protocol::CLOSE);
                        windows[used - 1].as_mut().unwrap().closing = Some(sys::ticks());
                    } else {
                        let _ = sys::kill(w.pid);
                        let _ = sys::waitpid(w.pid);
                        used -= 1;
                        windows[used] = None;
                        display.release_cache(w.cache_slot);
                        scene_dirty = true;
                        focused = if used > 0 {
                            windows[used - 1].unwrap().pid
                        } else {
                            shell_pid.unwrap_or(0)
                        };
                        focus(focused);
                    }
                } else if w.resizable && hit(mx, my, w.x + w.width as i32 - 66, w.y, 33, TITLE_H) {
                    let window = windows[used - 1].as_mut().unwrap();
                    let target = window.expansion.toggle(
                        window.bounds(),
                        crate::geometry::Workspace {
                            width: info.width,
                            height: info.height,
                            top: usize::from(work.top),
                            bottom: usize::from(work.bottom),
                        }
                        .maximized(),
                    );
                    window.requested = Some(target);
                } else if !w.expansion.expanded()
                    && w.offered.is_none()
                    && w.requested.is_none()
                    && hit(
                        mx,
                        my,
                        w.x,
                        w.y,
                        w.width as i32 - if w.resizable { 66 } else { 33 },
                        TITLE_H,
                    )
                {
                    drag = Some((mx - w.x, my - w.y));
                    println!("[desktop] drag started: pid={}", w.pid);
                }
            } else {
                focused = shell_pid.unwrap_or(0);
                focus(focused);
                scene_dirty = true;
            }
        }
        if let Some((pid, start, edges, x, y)) = resize {
            if let Some(window) = windows[..used].iter_mut().flatten().find(|w| w.pid == pid) {
                let target = crate::geometry::Workspace {
                    width: info.width,
                    height: info.height,
                    top: usize::from(work.top),
                    bottom: usize::from(work.bottom),
                }
                .resized(start, edges, mx - x, my - y, window.minimum);
                if target != window.bounds() {
                    window.requested = Some(target);
                }
            }
        }
        if !down {
            drag = None;
            resize = None;
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
        let mut changed = 0u32;
        for (index, slot) in windows[..used].iter_mut().enumerate() {
            let Some(window) = slot else {
                continue;
            };
            let before = window.bounds();
            if display.update_surface(window) {
                compose = true;
                changed |= 1 << index;
                scene_dirty |= before != window.bounds();
            }
            if window.resizable && window.offered.is_none() {
                if let Some(bounds) = window.requested.take() {
                    if (bounds.width, bounds.height) == (window.width, window.height) {
                        // A mode toggle can keep the same extents (for example
                        // a manually resized work-area-sized window).
                        window.expansion.offered();
                        window.expansion.committed();
                        scene_dirty = true;
                        (window.x, window.y) = (bounds.x, bounds.y);
                        compose |= scene_dirty;
                    } else if display.prepare_resize(window.cache_slot, bounds.width, bounds.height)
                    {
                        window.expansion.offered();
                        window.offered = Some(bounds);
                        window.resize_sent = false;
                    } else {
                        window.expansion.refused();
                        println!("[compositor] resize allocation refused");
                    }
                }
            }
            if let Some(bounds) = window.offered {
                if !window.resize_sent
                    && sys::send_message(
                        window.pid,
                        protocol::resize_message(bounds.width, bounds.height),
                    )
                    .is_ok()
                {
                    window.resize_sent = true;
                }
            }
            // Configuration replies precede pointer events; frame release follows presentation.
            if let Some(reply) = window.reply {
                if reply & protocol::KIND != protocol::RELEASE
                    && sys::send_message(window.pid, reply).is_ok()
                {
                    window.reply = None;
                }
            }
            if let Some(focused) = window.focus_event {
                if sys::send_message(window.pid, protocol::FOCUS | u32::from(focused)).is_ok() {
                    window.focus_event = None;
                }
            }
            if window.native
                && window.pid == focused
                && drag.is_none()
                && resize.is_none()
                && !decoration_capture
            {
                let limit = if window.resizable { 2048 } else { 1024 };
                let pointer = (
                    (mx - window.x).clamp(-limit, limit - 1),
                    (my - window.y - TITLE_H).clamp(-limit, limit - 1),
                    buttons,
                );
                if window.pointer != Some(pointer) {
                    let payload = if window.resizable {
                        protocol::pointer(pointer.0, pointer.1, buttons)
                    } else {
                        protocol::POINTER
                            | (pointer.0 as u32 & 2047)
                            | ((pointer.1 as u32 & 2047) << 11)
                            | (u32::from(buttons & 7) << 22)
                    };
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
                scene_dirty = true;
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
                        scene_dirty = true;
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
                if shell_connected
                    && (!over_window || desktop_capture)
                    && drag.is_none()
                    && resize.is_none()
                {
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
            decoration_capture = false;
        }
        if compose {
            let mut bounds = (info.width, info.height, 0, 0);
            let mut paint_bounds = [(0, 0, 0, 0); MAX];
            for (index, slot) in windows[..used].iter().enumerate() {
                let Some(window) = slot else {
                    continue;
                };
                paint_bounds[index] = (
                    (window.x - 12).max(0) as usize,
                    (window.y - 8).max(0) as usize,
                    (window.x as usize + window.width + 12).min(info.width),
                    (window.y as usize + window.height + TITLE_H as usize + 16).min(info.height),
                );
                bounds.0 = bounds.0.min((window.x - 12).max(0) as usize);
                bounds.1 = bounds.1.min((window.y - 8).max(0) as usize);
                bounds.2 = bounds
                    .2
                    .max((window.x as usize + window.width + 12).min(info.width));
                bounds.3 = bounds.3.max(
                    (window.y as usize + window.height + TITLE_H as usize + 16).min(info.height),
                );
            }
            let isolated = crate::damage::isolated_content(
                &paint_bounds[..used],
                changed,
                scene_dirty
                    || focused != announced_focus
                    || windows[..used].iter().flatten().any(|window| {
                        18 + crate::display::string(&window.title).len() * 8 > window.width
                    }),
            );
            if let Some((index, region)) = isolated {
                // Other windows neither paint nor overlap this restored region.
                // Their cached scene remains valid; this window's backdrop comes
                // from the shell and unchanged pixels outside its paint bounds.
                display.restore_background(region);
                let window = windows[index].as_ref().expect("isolated live window");
                display.window(window, window.pid == focused, true);
            } else {
                // Movement/focus/background changes and overlapping glass keep
                // complete bottom-to-top backdrop recomposition.
                display.restore_background((
                    old_bounds.0.min(bounds.0),
                    old_bounds.1.min(bounds.1),
                    old_bounds.2.max(bounds.2),
                    old_bounds.3.max(bounds.3),
                ));
                for window in windows[..used].iter().flatten() {
                    display.window(window, window.pid == focused, false);
                }
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
        // The single-buffer client uses RELEASE as its frame pacing signal.
        // Publish after presentation so its next draw samples current app state
        // instead of queuing another stale frame while we compose this one.
        // Snapshot ownership was already transferred; no user reference is held.
        for window in windows[..used].iter_mut().flatten() {
            if let Some(reply) = window.reply {
                if reply & protocol::KIND == protocol::RELEASE
                    && sys::send_message(window.pid, reply).is_ok()
                {
                    window.reply = None;
                }
            }
        }
        old_mouse = Some((mx, my));
        previous = buttons;
        if !display.pointer_input.pending() {
            sys::wait_events(sys::Events::MAILBOX | sys::Events::POINTER, 1)
                .expect("desktop input/message/deadline wait");
        }
    }
}
