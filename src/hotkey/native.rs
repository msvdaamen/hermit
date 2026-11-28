//! macOS and Windows backend: a direct registration through the `global-hotkey` crate.

use futures::StreamExt;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::{App, Task};

/// Option+Space on macOS and Alt+Space on Windows, as Raycast uses. On Windows this
/// takes over the window menu shortcut (as PowerToys Run does).
const MODIFIERS: Modifiers = Modifiers::ALT;

/// Must run on the main thread: macOS requires it, and on Windows the hotkey
/// messages arrive on the thread that created the manager.
pub(super) fn listen(
    cx: &mut App,
    on_press: impl Fn(&mut App) + 'static,
) -> anyhow::Result<Task<()>> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = HotKey::new(Some(MODIFIERS), Code::Space);
    manager.register(hotkey)?;
    tracing::info!("global hotkey registered: {hotkey}");

    // The handler runs on the crate's event thread; forward presses to the main thread.
    let (sender, mut presses) = futures::channel::mpsc::unbounded();
    let hotkey_id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id() == hotkey_id && event.state() == HotKeyState::Pressed {
            // Fails only once the listener task is gone, when presses are moot.
            sender.unbounded_send(()).ok();
        }
    }));

    Ok(cx.spawn(async move |cx| {
        // The grab lasts as long as the manager, so the task owns it.
        let _manager = manager;
        while presses.next().await.is_some() {
            cx.update(|cx| on_press(cx));
        }
    }))
}
