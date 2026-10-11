//! Monitor Lizard core: find monitors and talk to them over DDC/CI.
//!
//! The command-line tool (`main.rs`) and the future UI both use this crate,
//! so all the Windows-specific code lives here and nowhere else.

pub mod display;
#[cfg(windows)]
mod laptop;
#[cfg(not(windows))]
#[path = "laptop_stub.rs"]
mod laptop;
pub mod monitor;
mod names;
pub mod vcp;

pub use monitor::{Monitor, Monitors};

/// Built-in screen brightness check, for troubleshooting (`doctor` command).
pub fn builtin_screen_check() -> Result<u8, String> {
    laptop::get()
}
