//! Linux backend: a StatusNotifierItem served over D-Bus by `ksni`.

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::{App, Task};
use ksni::TrayMethods as _;
use ksni::menu::StandardItem;

use super::TrayAction;
use crate::icon::RgbaImage;

/// The tray item. `ksni` calls into it from its own thread, so it only forwards
/// actions to the main thread.
struct HermitTray {
    icon: ksni::Icon,
    actions: UnboundedSender<TrayAction>,
}

impl HermitTray {
    fn send(&self, action: TrayAction) {
        // Fails only once the listener task is gone, when actions are moot.
        self.actions.unbounded_send(action).ok();
    }
}

impl ksni::Tray for HermitTray {
    fn id(&self) -> String {
        "hermit".to_owned()
    }

    fn title(&self) -> String {
        "Hermit".to_owned()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "Hermit".to_owned(),
            ..ksni::ToolTip::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayAction::Open);
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open Hermit".to_owned(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayAction::Open)),
                ..StandardItem::default()
            }
            .into(),
            ksni::MenuItem::Separator,
            StandardItem {
                label: "Quit".to_owned(),
                icon_name: "application-exit".to_owned(),
                activate: Box::new(|tray: &mut Self| tray.send(TrayAction::Quit)),
                ..StandardItem::default()
            }
            .into(),
        ]
    }
}

pub(super) fn spawn(
    cx: &mut App,
    icon: RgbaImage,
    on_action: impl Fn(TrayAction, &mut App) + 'static,
) -> Task<()> {
    let (sender, mut actions) = unbounded();
    let tray = HermitTray {
        icon: to_ksni_icon(icon),
        actions: sender,
    };

    cx.spawn(async move |cx| {
        let handle = match tray.spawn().await {
            Ok(handle) => handle,
            Err(error) => {
                log::error!("could not show the tray icon: {error}");
                return;
            }
        };

        while let Some(action) = actions.next().await {
            cx.update(|cx| on_action(action, cx));
        }

        // Keep the tray service alive while actions can arrive.
        drop(handle);
    })
}

/// StatusNotifierItem pixmaps are ARGB32 in network byte order.
fn to_ksni_icon(image: RgbaImage) -> ksni::Icon {
    let mut data = image.pixels;
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1); // [r, g, b, a] -> [a, r, g, b]
    }
    ksni::Icon {
        width: image.width as i32,
        height: image.height as i32,
        data,
    }
}
