//! The launcher window: opened and closed by the global hotkey.

use std::time::{Duration, Instant};

use gpui_kit::component::ActiveTheme;
use gpui_kit::*;
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram};

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
    metrics: LauncherMetrics,
}

impl Global for LauncherWindow {}

/// The launcher's instruments, created once from the global meter provider (a
/// no-op when telemetry is off).
#[derive(Clone)]
struct LauncherMetrics {
    opens: Counter<u64>,
    open_duration: Histogram<f64>,
    visible_duration: Histogram<f64>,
}

impl LauncherMetrics {
    fn new() -> Self {
        let meter = opentelemetry::global::meter("hermit");
        Self {
            opens: meter
                .u64_counter("hermit.launcher.opens")
                .with_description("Times the launcher window was opened")
                .build(),
            open_duration: meter
                .f64_histogram("hermit.launcher.open.duration")
                .with_description("Time to create the launcher window")
                .with_unit("s")
                .with_boundaries(vec![
                    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 1.0, 2.5,
                ])
                .build(),
            visible_duration: meter
                .f64_histogram("hermit.launcher.visible.duration")
                .with_description("How long the launcher stayed open")
                .with_unit("s")
                .with_boundaries(vec![0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0, 1800.0])
                .build(),
        }
    }
}

pub fn init(cx: &mut App) {
    cx.set_global(LauncherWindow {
        handle: None,
        metrics: LauncherMetrics::new(),
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

    let span = tracing::info_span!(
        "launcher.show",
        trigger = trigger.as_str(),
        window.kind = tracing::field::Empty,
    )
    .entered();
    let metrics = cx.global::<LauncherWindow>().metrics.clone();
    let started = Instant::now();
    match open(metrics.visible_duration, cx) {
        Ok((handle, kind)) => {
            span.record("window.kind", kind);
            let attributes = [
                KeyValue::new("trigger", trigger.as_str()),
                KeyValue::new("window.kind", kind),
            ];
            metrics.opens.add(1, &attributes);
            metrics
                .open_duration
                .record(started.elapsed().as_secs_f64(), &attributes);
            tracing::debug!("launcher opened");
            cx.global_mut::<LauncherWindow>().handle = Some(handle);
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
fn open(visible_duration: Histogram<f64>, cx: &mut App) -> Result<(AnyWindowHandle, &'static str)> {
    let build = move |window: &mut Window, cx: &mut App| {
        cx.new(|cx| LauncherView::new(visible_duration, window, cx))
    };

    // Not every compositor implements layer shell (GNOME does not).
    #[cfg(target_os = "linux")]
    match open_window(window_options(layer_shell_kind(), cx), cx, build.clone()) {
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
    opened_at: Instant,
    /// Set when the view closes its own window; anything else is [`CloseReason::Toggled`].
    close_reason: Option<CloseReason>,
    visible_duration: Histogram<f64>,
    /// Set once the window has gained focus, so the deactivation that can precede
    /// the first activation does not close the window immediately.
    was_active: bool,
    _activation: Subscription,
}

impl LauncherView {
    fn new(visible_duration: Histogram<f64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

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
            opened_at: Instant::now(),
            close_reason: None,
            visible_duration,
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
        let visible: Duration = self.opened_at.elapsed();
        self.visible_duration.record(
            visible.as_secs_f64(),
            &[KeyValue::new("close.reason", reason.as_str())],
        );
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
