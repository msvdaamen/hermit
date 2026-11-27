//! The system tray (notification area) icon, with a menu to open or quit Hermit.
//!
//! - Linux: a StatusNotifierItem over D-Bus (`ksni`), shown by KDE Plasma and other
//!   desktops with a StatusNotifierItem host. Plain GNOME needs the AppIndicator extension.
//! - macOS and Windows: the `tray-icon` crate.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native;

use gpui_kit::{App, Task};

/// What the user picked from the tray icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// Show the launcher (left click, or "Open Hermit" in the menu).
    Open,
    /// Quit the application.
    Quit,
}

/// Keeps the tray icon visible; dropping it removes the icon.
#[must_use = "the tray icon is removed when this is dropped"]
pub struct TrayIcon {
    _task: Task<()>,
}

/// Shows the tray icon and calls `on_action` on the main thread for each user action.
pub fn spawn(
    cx: &mut App,
    on_action: impl Fn(TrayAction, &mut App) + 'static,
) -> anyhow::Result<TrayIcon> {
    let icon = crate::icon::app_icon()?;

    #[cfg(target_os = "linux")]
    return Ok(TrayIcon {
        _task: linux::spawn(cx, icon, on_action),
    });

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    return native::spawn(cx, icon, on_action).map(|task| TrayIcon { _task: task });

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (cx, icon, on_action);
        anyhow::bail!("tray icons are not supported on this platform")
    }
}
