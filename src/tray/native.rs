//! macOS and Windows backend: the `tray-icon` crate.

use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui_kit::{App, Task};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use super::TrayAction;
use crate::icon::RgbaImage;

/// Must run on the main thread, which owns the platform event loop the icon lives on.
pub(super) fn spawn(
    cx: &mut App,
    icon: RgbaImage,
    on_action: impl Fn(TrayAction, &mut App) + 'static,
) -> anyhow::Result<Task<()>> {
    let open = MenuItem::new("Open Hermit", true, None);
    let quit = MenuItem::new("Quit Hermit", true, None);
    let menu = Menu::new();
    menu.append(&open)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Hermit")
        .with_icon(Icon::from_rgba(icon.pixels, icon.width, icon.height)?);
    // Windows convention: left click acts, right click opens the menu.
    // macOS convention: any click opens the menu.
    #[cfg(target_os = "windows")]
    let builder = builder.with_menu_on_left_click(false);
    let tray_icon = builder.build()?;

    // Both handlers run on the crate's event thread; forward to the main thread.
    let (sender, mut actions) = unbounded();
    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    #[cfg(target_os = "windows")]
    let click_sender = sender.clone();
    MenuEvent::set_event_handler(Some(Arc::new(move |event: MenuEvent| {
        let action = if event.id == open_id {
            TrayAction::Open
        } else if event.id == quit_id {
            TrayAction::Quit
        } else {
            return;
        };
        // Fails only once the listener task is gone, when actions are moot.
        sender.unbounded_send(action).ok();
    })));

    #[cfg(target_os = "windows")]
    tray_icon::TrayIconEvent::set_event_handler(Some(move |event| {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            click_sender.unbounded_send(TrayAction::Open).ok();
        }
    }));

    Ok(cx.spawn(async move |cx| {
        // The icon (and its menu) disappear when dropped, so the task owns them.
        let _tray_icon = tray_icon;
        let _menu_items = (open, quit);
        while let Some(action) = actions.next().await {
            cx.update(|cx| on_action(action, cx));
        }
    }))
}
