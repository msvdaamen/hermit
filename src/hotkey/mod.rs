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

/// Registers the global hotkey and calls `on_press` on the main thread each time it
/// is pressed. When the hotkey cannot be registered, logs why and calls
/// `on_unavailable` once on the main thread; on Linux that happens later, once the
/// desktop portal has answered.
pub fn listen(
    cx: &mut App,
    on_press: impl Fn(&mut App) + 'static,
    on_unavailable: impl FnOnce(&mut App) + 'static,
) -> HotkeyListener {
    #[cfg(target_os = "linux")]
    return HotkeyListener {
        _task: portal::listen(cx, on_press, on_unavailable),
    };

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return match native::listen(cx, on_press) {
        Ok(task) => HotkeyListener { _task: task },
        Err(error) => {
            tracing::error!("could not register the global hotkey: {error:#}");
            on_unavailable(cx);
            HotkeyListener {
                _task: Task::ready(()),
            }
        }
    };

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = on_press;
        tracing::error!("global hotkeys are not supported on this platform");
        on_unavailable(cx);
        HotkeyListener {
            _task: Task::ready(()),
        }
    }
}
