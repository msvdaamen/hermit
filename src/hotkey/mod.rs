//! The system-wide hotkey that summons the launcher.
//!
//! - Linux (Wayland only): the XDG desktop portal `GlobalShortcuts` interface. The
//!   compositor owns the binding, so the user may be asked to confirm it and can
//!   change it in the desktop's shortcut settings.
//! - macOS and Windows: the `global-hotkey` crate, which registers the key directly.

#[cfg(target_os = "linux")]
mod desktop_entry;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native;
#[cfg(target_os = "linux")]
mod portal;

use gpui_kit::{App, Task};

/// Keeps the hotkey registered; dropping it stops listening.
#[must_use = "the hotkey is unregistered when the listener is dropped"]
pub struct HotkeyListener {
    _task: Task<()>,
}

/// Registers the global hotkey and calls `on_press` on the main thread each time it is pressed.
pub fn listen(
    cx: &mut App,
    on_press: impl Fn(&mut App) + 'static,
) -> anyhow::Result<HotkeyListener> {
    #[cfg(target_os = "linux")]
    return Ok(HotkeyListener {
        _task: portal::listen(cx, on_press),
    });

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return native::listen(cx, on_press).map(|task| HotkeyListener { _task: task });

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (cx, on_press);
        anyhow::bail!("global hotkeys are not supported on this platform")
    }
}
