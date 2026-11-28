//! Quits cleanly when the process is asked to stop: by SIGTERM (which a
//! development build sends the instance it replaces, as does a session logout)
//! or SIGINT (Ctrl+C in a terminal). Their default action ends the process at
//! once, before telemetry is flushed and the release-health session is closed.

use gpui_kit::App;

#[cfg(unix)]
pub fn quit_on_signal(cx: &mut App) {
    use async_signal::{Signal, Signals};
    use futures::StreamExt as _;

    let mut signals = match Signals::new([Signal::Term, Signal::Int]) {
        Ok(signals) => signals,
        Err(error) => {
            tracing::warn!("could not handle termination signals: {error}");
            return;
        }
    };

    // From here on these signals no longer end the process by themselves, so a
    // quit that hangs needs SIGKILL. Quitting waits at most for telemetry to flush.
    cx.spawn(async move |cx| match signals.next().await {
        Some(Ok(signal)) => {
            tracing::info!(?signal, "quitting on signal");
            cx.update(|cx| cx.quit());
        }
        Some(Err(error)) => tracing::warn!("stopped listening for termination signals: {error}"),
        None => {}
    })
    .detach();
}

/// Windows has no SIGTERM; the console's Ctrl+C keeps its default action.
#[cfg(not(unix))]
pub fn quit_on_signal(_: &mut App) {}
