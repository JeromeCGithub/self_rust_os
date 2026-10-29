//! A complete glass application using only the public Ring 3 UI toolkit.
#![no_std]
#![no_main]

use userlib::{
    println, rsx, sys,
    ui::{Color, Error, Material, Style, View, Window, WindowOptions},
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
    verify_desktop_role();
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
    window
        .mount(State::default(), app, update)
        .expect("glass component loop");
    sys::exit(0);
}

#[derive(Default)]
struct State {
    changes: u32,
}
#[derive(Clone)]
enum Action {
    ChangeAccent,
}

fn app_style() -> Style {
    Style::new().padding(24)
}

fn app(state: &State) -> impl View<Action> {
    rsx! {
        column { style: app_style(),
            heading { "Hello, glass." }
            label { "A little clarity. A little depth." }
            label { "Your app draws content. The desktop does the rest." }
            space { 16 }
            button { key: "accent", onclick: Action::ChangeAccent,
                if state.changes == 0 { "Change accent" } else { "Change again" }
            }
            space { 12 }
            { appearance_card(state) }
        }
    }
}

fn appearance_card(state: &State) -> impl View<Action> {
    let lavender = state.changes % 2 != 0;
    rsx! {
        column { style: Style::new().color(Color::rgb(if lavender { 0xd2b9ff } else { 0x9dd6ff })),
            card { title: if lavender { "SOFT LAVENDER" } else { "COOL BLUE" },
                "Glass behind. Sharp, readable content above."
            }
            label { "RUSTOS  /  DECLARATIVE COMPONENTS" }
        }
    }
}

fn update(state: &mut State, action: Action) {
    match action {
        Action::ChangeAccent => {
            state.changes = state.changes.wrapping_add(1);
            println!("[hello-ui] accent changed: {}", state.changes);
        }
    }
}

fn verify_desktop_role() {
    use userlib::ui::{desktop_protocol, protocol};
    sys::send_message(0, desktop_protocol::DESKTOP).expect("role probe");
    let start = sys::ticks();
    loop {
        if let Some((sender, message)) = sys::receive_message().expect("role reply") {
            assert_eq!(sender, 0);
            assert_eq!(
                message,
                protocol::REJECT,
                "ordinary apps cannot claim the desktop role"
            );
            println!("[hello-ui] desktop role denied");
            break;
        }
        assert!(
            sys::ticks().wrapping_sub(start) < 36,
            "desktop role probe timeout"
        );
        sys::sleep_ticks(1).expect("role wait");
    }
}
