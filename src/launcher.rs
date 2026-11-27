//! The launcher window: opened and closed by the global hotkey.

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

actions!(launcher, [Dismiss]);

const CONTEXT: &str = "Launcher";

/// The launcher window, if one has been opened. The handle may be stale when the
/// window closed itself (Escape, focus loss); [`toggle`] and [`show`] handle that.
#[derive(Default)]
struct LauncherWindow(Option<AnyWindowHandle>);

impl Global for LauncherWindow {}

pub fn init(cx: &mut App) {
    cx.set_global(LauncherWindow::default());
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

/// Opens the launcher, or closes it when it is already open.
pub fn toggle(cx: &mut App) {
    if !close(cx) {
        show(cx);
    }
}

/// Opens the launcher unless it is already open.
pub fn show(cx: &mut App) {
    // `update` fails when the window has closed itself; the handle is stale then.
    let is_open = cx
        .global::<LauncherWindow>()
        .0
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_ok());
    if is_open {
        return;
    }

    match open(cx) {
        Ok(handle) => {
            log::debug!("launcher opened");
            cx.global_mut::<LauncherWindow>().0 = Some(handle);
        }
        Err(error) => log::error!("failed to open the launcher window: {error:#}"),
    }
}

/// Closes the launcher. Returns whether it was open.
fn close(cx: &mut App) -> bool {
    let Some(handle) = cx.global_mut::<LauncherWindow>().0.take() else {
        return false;
    };
    let closed = handle
        .update(cx, |_, window, _| window.remove_window())
        .is_ok();
    if closed {
        log::debug!("launcher closed");
    }
    closed
}

fn open(cx: &mut App) -> Result<AnyWindowHandle> {
    let build = |window: &mut Window, cx: &mut App| cx.new(|cx| LauncherView::new(window, cx));

    // Not every compositor implements layer shell (GNOME does not).
    #[cfg(target_os = "linux")]
    match open_window(window_options(layer_shell_kind(), cx), cx, build) {
        Ok((handle, _)) => return Ok(handle),
        Err(error) => log::warn!("layer-shell window unavailable, using a popup window: {error:#}"),
    }

    open_window(window_options(WindowKind::PopUp, cx), cx, build).map(|(handle, _)| handle)
}

fn window_options(kind: WindowKind, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(None, size(px(720.), px(420.)), cx)),
        titlebar: None,
        kind,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some(crate::APP_ID.to_owned()),
        ..WindowOptions::default()
    }
}

/// A Wayland overlay surface: centered above all windows, with exclusive keyboard focus.
/// Regular Wayland windows can neither position themselves nor reliably take focus.
#[cfg(target_os = "linux")]
fn layer_shell_kind() -> WindowKind {
    use gpui_kit::layer_shell::{Anchor, KeyboardInteractivity, Layer, LayerShellOptions};

    WindowKind::LayerShell(LayerShellOptions {
        namespace: "hermit".to_owned(),
        layer: Layer::Overlay,
        anchor: Anchor::empty(),
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
        ..LayerShellOptions::default()
    })
}

struct LauncherView {
    focus_handle: FocusHandle,
    /// Set once the window has gained focus, so the deactivation that can precede
    /// the first activation does not close the window immediately.
    was_active: bool,
    _activation: Subscription,
}

impl LauncherView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        // Close when the user clicks or switches away, like other launchers.
        let activation = cx.observe_window_activation(window, |this, window, _| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                window.remove_window();
            }
        });

        Self {
            focus_handle,
            was_active: false,
            _activation: activation,
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }
}

impl Render for LauncherView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_xl()
            .child("Hello, World!")
    }
}
