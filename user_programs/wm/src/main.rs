// Ring 3 compositor entry point. Desktop policy lives in the separate DE process.
mod damage;
mod display;
mod glass;
mod session;
mod window;

fn main() {
    session::run();
}
