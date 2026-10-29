//! Run with rustc --edition=2021 --test .../ui/host_tests.rs.
pub mod declarative;
mod input;
pub mod protocol;
pub mod raster;
mod ui {
    pub use super::declarative;
}
