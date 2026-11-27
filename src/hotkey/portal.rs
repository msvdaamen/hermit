//! Wayland backend: the XDG desktop portal `GlobalShortcuts` interface.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::{AppID, register_host_app_with_connection, zbus};
use futures::StreamExt;
use gpui_kit::{App, AsyncApp, Task};

const SHORTCUT_ID: &str = "toggle-launcher";
/// Trigger syntax from the XDG "shortcuts" specification. The compositor treats it
/// as a suggestion: the user may pick another key when confirming the binding.
const PREFERRED_TRIGGER: &str = "LOGO+space";

pub(super) fn listen(cx: &mut App, on_press: impl Fn(&mut App) + 'static) -> Task<()> {
    cx.spawn(async move |cx| {
        if let Err(error) = run(cx, &on_press).await {
            log::error!("global shortcut portal failed: {error:#}");
        }
    })
}

async fn run(cx: &mut AsyncApp, on_press: &impl Fn(&mut App)) -> anyhow::Result<()> {
    // A dedicated connection: the host app registration must be the first portal
    // call on a connection, and GPUI already uses the shared one for settings.
    let connection = zbus::Connection::session().await?;
    // Registration resolves the app id to its desktop entry, so install that first.
    if let Err(error) = super::desktop_entry::ensure_installed() {
        log::warn!("could not install the desktop entry: {error:#}");
    }
    let app_id: AppID = crate::APP_ID.parse()?;
    if let Err(error) = register_host_app_with_connection(connection.clone(), app_id).await {
        // Older portals lack the registry; some desktops then still bind the shortcut
        // under an empty app id, while KDE refuses it with "An app id is required".
        log::warn!("could not register with the desktop portal: {error}");
    }

    let portal = GlobalShortcuts::with_connection(connection).await?;
    let session = portal.create_session(Default::default()).await?;
    let shortcut =
        NewShortcut::new(SHORTCUT_ID, "Open Hermit").preferred_trigger(PREFERRED_TRIGGER);
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None, Default::default())
        .await?
        .response()?;

    match bound
        .shortcuts()
        .iter()
        .find(|shortcut| shortcut.id() == SHORTCUT_ID)
    {
        Some(shortcut) => log::info!(
            "global shortcut bound to {} (change it in the desktop's shortcut settings)",
            shortcut.trigger_description()
        ),
        None => log::warn!("the desktop did not bind the global shortcut"),
    }

    let mut activations = portal.receive_activated().await?;
    while let Some(activation) = activations.next().await {
        if activation.shortcut_id() == SHORTCUT_ID {
            cx.update(|cx| on_press(cx));
        }
    }

    // Keep the session open until the signal stream ends.
    drop(session);
    Ok(())
}
