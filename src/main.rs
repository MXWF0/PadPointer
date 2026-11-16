#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod config;
mod gamepad;
mod input;
mod keys;
mod mapping;
mod mode;
mod system;
mod worker;
slint::include_modules!();

fn main() {
    // SAFETY: Set this before Slint starts and before any worker threads exist.
    // Recreate the native window and software surface when returning from the tray.
    unsafe { std::env::set_var("SLINT_DESTROY_WINDOW_ON_HIDE", "1") };
    if let Err(error) = app::run() {
        tracing::error!(%error, "PadPointer failed");
        system::message(&format!("PadPointer 无法完成操作。\n\n{error:#}"));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
