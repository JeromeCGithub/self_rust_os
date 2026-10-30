// Ring 3 desktop lifecycle and event loop.
mod applications;
mod view;
mod wallpaper;
use applications::{discover, EMPTY_APP, MAX};
use userlib::{
    println, sys,
    ui::{DesktopSurface, Event},
};
use view::Painter;
fn main() {
    let info = sys::fb_info().expect("desktop geometry");
    let mut surface = DesktopSurface::connect().expect("desktop role");
    surface
        .reserve_work_area(28, 32)
        .expect("desktop workspace");
    let wallpaper = wallpaper::load();
    println!("[desktop] wallpaper loaded: {}", wallpaper.is_some());
    // Publish the wallpaper promptly, then the discovered shell controls. The
    // second draw must wait for the first cached copy's RELEASE before writing.
    surface
        .draw(|canvas| {
            Painter { canvas, info }.background(wallpaper);
        })
        .expect("wallpaper frame");
    let mut apps = [EMPTY_APP; MAX];
    let count = discover(&mut apps);
    surface
        .draw(|canvas| {
            Painter { canvas, info }.desktop(&apps[..count], wallpaper);
        })
        .expect("desktop frame");
    println!("[desktop] ready: {} applications", count);
    let mut previous = 0;
    loop {
        for _ in 0..16 {
            match surface.poll_event().expect("desktop events") {
                Some(Event::Pointer { x, y, buttons }) => {
                    let click = buttons & 1 != 0 && previous & 1 == 0;
                    previous = buttons;
                    if click && (40..120).contains(&y) && x >= 18 {
                        let index = ((x - 18) / 100) as usize;
                        if index < count {
                            let app = apps[index];
                            app.launch(&mut surface);
                        }
                    }
                }
                Some(Event::CloseRequested) => sys::exit(0),
                _ => {}
            }
        }
        sys::sleep_ticks(1).expect("desktop idle");
    }
}
