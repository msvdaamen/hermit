// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod hotkey;
mod launcher;

use gpui_kit::*;

/// Reverse-DNS application id, shared by the window and the desktop portal.
pub const APP_ID: &str = "dev.hermit.Hermit";

/// Keeps the global hotkey registered for the lifetime of the application.
struct HotkeyRegistration {
    _listener: hotkey::HotkeyListener,
}

impl Global for HotkeyRegistration {}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("hermit=info"))
        .init();

    // GPUI is built with its Wayland backend only; without a compositor it would
    // silently start headless, with no way to show the launcher.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|display| display.is_empty()) {
        log::error!(
            "Hermit needs a Wayland session (WAYLAND_DISPLAY is not set); X11 is not supported."
        );
        std::process::exit(1);
    }

    application()
        .with_assets(assets::Assets)
        // Hermit lives in the background: closing the launcher must not quit the app.
        .with_quit_mode(QuitMode::Explicit)
        .run(|cx| {
            init(cx);
            launcher::init(cx);

            match hotkey::listen(cx, launcher::toggle) {
                Ok(listener) => {
                    log::info!("Hermit is running in the background.");
                    cx.set_global(HotkeyRegistration {
                        _listener: listener,
                    });
                }
                Err(error) => {
                    log::error!("could not register the global hotkey: {error:#}");
                    // Without a hotkey there is no other way in, so show the launcher now.
                    launcher::toggle(cx);
                }
            }
        });
}
