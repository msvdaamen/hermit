//! The launcher window: opened and closed by the global hotkey.

use std::time::Instant;

use gpui_kit::base::input::InputState;
use gpui_kit::base::v_flex;
use gpui_kit::*;

use crate::components::CommandInput;
use crate::telemetry::metrics;
use crate::theme::ActiveTheme;

actions!(launcher, [Dismiss]);

const CONTEXT: &str = "Launcher";

/// What asked for the launcher. Recorded on its spans and metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The global hotkey.
    Hotkey,
    /// The tray icon or its menu.
    Tray,
    /// Startup, when no hotkey could be registered.
    Startup,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Hotkey => "hotkey",
            Self::Tray => "tray",
            Self::Startup => "startup",
        }
    }
}

/// How the launcher was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseReason {
    Escape,
    FocusLost,
    /// Closed from outside: the hotkey toggled it, or Hermit quit.
    Toggled,
}

impl CloseReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Escape => "escape",
            Self::FocusLost => "focus_lost",
            Self::Toggled => "toggled",
        }
    }
}

/// The launcher window, if one has been opened. The handle may be stale when the
/// window closed itself (Escape, focus loss); [`toggle`] and [`show`] handle that.
struct LauncherWindow {
    handle: Option<AnyWindowHandle>,
    /// Whether a launcher window has opened since Hermit started.
    has_opened: bool,
}

impl Global for LauncherWindow {}

pub fn init(cx: &mut App) {
    cx.set_global(LauncherWindow {
        handle: None,
        has_opened: false,
    });
    cx.bind_keys([KeyBinding::new("escape", Dismiss, Some(CONTEXT))]);
}

/// Opens the launcher, or closes it when it is already open.
pub fn toggle(trigger: Trigger, cx: &mut App) {
    if !close(cx) {
        show(trigger, cx);
    }
}

/// Opens the launcher unless it is already open.
pub fn show(trigger: Trigger, cx: &mut App) {
    // `update` fails when the window has closed itself; the handle is stale then.
    let is_open = cx
        .global::<LauncherWindow>()
        .handle
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_ok());
    if is_open {
        return;
    }

    // Every show creates a new window, but the first one also sets up the GPU
    // renderer and takes far longer. A separate span name keeps it from skewing
    // the timings of `launcher.show`.
    let is_first = !cx.global::<LauncherWindow>().has_opened;
    let span = if is_first {
        tracing::info_span!(
            "launcher.first_show",
            trigger = trigger.as_str(),
            window.kind = tracing::field::Empty,
        )
    } else {
        tracing::info_span!(
            "launcher.show",
            trigger = trigger.as_str(),
            window.kind = tracing::field::Empty,
        )
    }
    .entered();
    let started = Instant::now();
    match open(cx) {
        Ok((handle, kind)) => {
            span.record("window.kind", kind);
            let attributes = [
                ("trigger", trigger.as_str()),
                ("window.kind", kind),
                ("first_open", if is_first { "true" } else { "false" }),
            ];
            metrics::count("hermit.launcher.opens", &attributes);
            metrics::duration(
                "hermit.launcher.open.duration",
                started.elapsed(),
                &attributes,
            );
            tracing::debug!("launcher opened");
            let launcher = cx.global_mut::<LauncherWindow>();
            launcher.handle = Some(handle);
            launcher.has_opened = true;
        }
        Err(error) => tracing::error!("failed to open the launcher window: {error:#}"),
    }
}

/// Closes the launcher. Returns whether it was open.
fn close(cx: &mut App) -> bool {
    let Some(handle) = cx.global_mut::<LauncherWindow>().handle.take() else {
        return false;
    };
    let closed = handle
        .update(cx, |_, window, _| window.remove_window())
        .is_ok();
    if closed {
        tracing::debug!("launcher closed");
    }
    closed
}

/// Opens the window, returning it and its `window.kind` attribute.
fn open(cx: &mut App) -> Result<(AnyWindowHandle, &'static str)> {
    let build = |window: &mut Window, cx: &mut App| cx.new(|cx| LauncherView::new(window, cx));

    // Not every compositor implements layer shell (GNOME does not).
    #[cfg(target_os = "linux")]
    match open_window(window_options(layer_shell_kind(), cx), cx, build) {
        Ok((handle, _)) => return Ok((handle, "layer_shell")),
        Err(error) => {
            tracing::warn!("layer-shell window unavailable, using a popup window: {error:#}")
        }
    }

    open_window(window_options(WindowKind::PopUp, cx), cx, build)
        .map(|(handle, _)| (handle, "popup"))
}

fn window_options(kind: WindowKind, cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::centered(None, size(px(720.), px(420.)), cx)),
        titlebar: None,
        kind,
        // Lets the compositor show (and blur) what is behind the window; the view's
        // background alpha decides how much shows through.
        window_background: WindowBackgroundAppearance::Blurred,
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
    /// What the user is searching for.
    query: Entity<InputState>,
    opened_at: Instant,
    /// Set when the view closes its own window; anything else is [`CloseReason::Toggled`].
    close_reason: Option<CloseReason>,
    /// Set once the window has gained focus, so the deactivation that can precede
    /// the first activation does not close the window immediately.
    was_active: bool,
    _activation: Subscription,
}

impl LauncherView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        // The launcher opens ready to type into.
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search apps and commands…"));
        window.focus(&query.focus_handle(cx), cx);

        window.set_client_corner_radii(Corners::all(cx.theme().window_radius));

        // Close when the user clicks or switches away, like other launchers.
        let activation = cx.observe_window_activation(window, |this, window, _| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                this.close_reason = Some(CloseReason::FocusLost);
                window.remove_window();
            }
        });

        Self {
            focus_handle,
            query,
            opened_at: Instant::now(),
            close_reason: None,
            was_active: false,
            _activation: activation,
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, _: &mut Context<Self>) {
        self.close_reason = Some(CloseReason::Escape);
        window.remove_window();
    }
}

/// The view is released with its window, however that closed.
impl Drop for LauncherView {
    fn drop(&mut self) {
        let reason = self.close_reason.unwrap_or(CloseReason::Toggled);
        metrics::duration(
            "hermit.launcher.visible.duration",
            self.opened_at.elapsed(),
            &[("close.reason", reason.as_str())],
        );
    }
}

impl Render for LauncherView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .track_focus(&self.focus_handle)
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::dismiss))
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .rounded(cx.theme().window_radius)
            .child(
                div()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(CommandInput::new(&self.query)),
            )
            // Results go here.
            .child(div().flex_1())
    }
}
