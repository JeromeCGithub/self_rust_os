// Ring 3 compositor entry point. Desktop policy lives in the separate DE process.
mod cache;
mod damage;
mod display;
mod geometry;
mod glass;
mod input;
mod session;
mod window;

fn main() {
    session::run();
}
