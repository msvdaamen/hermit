// Release builds on Windows run without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod components;
mod hotkey;
mod icon;
mod launcher;
mod single_instance;
mod telemetry;
mod theme;
mod tray;

use gpui_kit::*;

/// Reverse-DNS application id, shared by the window and the desktop portal.
pub const APP_ID: &str = "dev.hermit.Hermit";

/// Keeps the global hotkey registered for the lifetime of the application.
struct HotkeyRegistration {
    _listener: hotkey::HotkeyListener,
}

impl Global for HotkeyRegistration {}

/// Keeps the tray icon visible for the lifetime of the application.
struct TrayRegistration {
    _icon: tray::TrayIcon,
}

impl Global for TrayRegistration {}

fn main() {
    let dotenv = cfg!(debug_assertions).then(dotenvy::dotenv);

    let telemetry = telemetry::init();

    match dotenv {
        Some(Ok(path)) => {
            tracing::info!("loaded settings from {}", telemetry::private_path(&path));
        }
        Some(Err(error)) if !error.not_found() => {
            tracing::warn!("could not load .env: {error}");
        }
        Some(Err(_)) | None => {}
    }

    // GPUI is built with its Wayland backend only; without a compositor it would
    // silently start headless, with no way to show the launcher.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none_or(|display| display.is_empty()) {
        tracing::error!(
            "Hermit needs a Wayland session (WAYLAND_DISPLAY is not set); X11 is not supported."
        );
        // `exit` skips destructors, so flush explicitly.
        drop(telemetry);
        std::process::exit(1);
    }

    // Held until the process exits. A lock that cannot be taken for another reason
    // (e.g. an unwritable directory) should not stop Hermit from starting.
    // A development build replaces the running instance, so a rebuild takes effect.
    let _instance_lock = match single_instance::acquire(cfg!(debug_assertions)) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            tracing::info!("Hermit is already running.");
            return;
        }
        Err(error) => {
            tracing::warn!("could not check for a running Hermit: {error:#}");
            None
        }
    };

    application()
        .with_assets(assets::Assets)
        // Hermit lives in the background: closing the launcher must not quit the app.
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            // Flush before the process exits; on macOS quitting never returns from `run`.
            // GPUI polls this future only after closing the windows, so the metrics the
            // launcher records when it closes are flushed too.
            let mut telemetry = Some(telemetry);
            cx.on_app_quit(move |_| {
                let telemetry = telemetry.take();
                async move {
                    if let Some(mut telemetry) = telemetry {
                        telemetry.shutdown();
                    }
                }
            })
            .detach();

            let _startup = tracing::info_span!("startup").entered();
            init(cx);
            theme::init(cx);
            launcher::init(cx);

            match tray::spawn(cx, |action, cx| match action {
                tray::TrayAction::Open => launcher::show(launcher::Trigger::Tray, cx),
                tray::TrayAction::Quit => cx.quit(),
            }) {
                Ok(icon) => cx.set_global(TrayRegistration { _icon: icon }),
                Err(error) => tracing::error!("could not show the tray icon: {error:#}"),
            }

            let listener = hotkey::listen(
                cx,
                |cx| launcher::toggle(launcher::Trigger::Hotkey, cx),
                // Without a hotkey there is no other way in, so show the launcher now.
                |cx| launcher::show(launcher::Trigger::Startup, cx),
            );
            cx.set_global(HotkeyRegistration {
                _listener: listener,
            });
            tracing::info!("Hermit is running in the background.");
        });
}
