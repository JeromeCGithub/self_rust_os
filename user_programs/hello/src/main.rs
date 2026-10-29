//! A complete glass application using only the public Ring 3 UI toolkit.
#![no_std]
#![no_main]

use userlib::{
    println, sys,
    ui::{Error, Material, Window, WindowOptions},
};

#[no_mangle]
pub extern "C" fn user_main() {
    println!("Hello from user space!");
    assert!(matches!(
        Window::create(WindowOptions::new("bad").content_size(0, 100)),
        Err(Error::InvalidGeometry)
    ));
    assert!(matches!(
        Window::create(WindowOptions::new("bad\n")),
        Err(Error::InvalidTitle)
    ));
    let window = Window::create(
        WindowOptions::new("Hello / Glass")
            .content_size(640, 400)
            .background(Material::GlassClear),
    )
    .expect("create glass window");
    assert!(matches!(
        Window::create(WindowOptions::new("second")),
        Err(Error::AlreadyConnected)
    ));
    println!("[hello-ui] invalid requests rejected");
    println!("[hello-ui] ready: 640x400 glass");
    let mut changes = 0u32;
    window
        .run(|ui| {
            ui.column(|ui| {
                ui.heading("Hello, glass.");
                ui.label("A little clarity. A little depth.");
                ui.label("Your app draws content. The desktop does the rest.");
                ui.space(16);
                if ui.button("Change accent").clicked() {
                    changes += 1;
                    println!("[hello-ui] accent changed: {}", changes);
                }
                ui.space(12);
                ui.card(
                    if changes % 2 == 0 {
                        "COOL BLUE"
                    } else {
                        "SOFT LAVENDER"
                    },
                    "Glass behind. Sharp, readable content above.",
                );
                ui.label("RUSTOS  /  NATIVE USERSPACE UI");
            });
        })
        .expect("glass event loop");
    sys::exit(0);
}
