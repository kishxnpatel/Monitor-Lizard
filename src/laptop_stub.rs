//! Non-Windows stand-in so the project type-checks on other systems.
pub fn available() -> bool { false }
pub fn get() -> Result<u8, String> { Err("Windows only".into()) }
pub fn set(_: u8) -> Result<(), String> { Err("Windows only".into()) }
