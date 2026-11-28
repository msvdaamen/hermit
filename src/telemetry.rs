//! Logging and Sentry: errors, crashes, traces, logs and metrics.
//!
//! Code records telemetry through `tracing`: spans become Sentry transactions and
//! spans, events become Sentry logs, and `error!` events also become Sentry issues.
//! Panics are reported as crashes. Metrics have no `tracing` API and go through
//! [`metrics`] instead.
//!
//! Hermit always prints its own log lines to stderr, filtered by `RUST_LOG`. It sends
//! telemetry to the first DSN it finds:
//!
//! 1. `SENTRY_DSN` in the environment at runtime (set but empty turns sending off);
//! 2. `HERMIT_SENTRY_DSN`, set in the environment when Hermit is compiled.
//!
//! Without either, nothing is sent.
//!
//! Sent data must not identify the user: log file system paths through
//! [`private_path`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use sentry::integrations::tracing::{ErrorLayer, LogLayer, SpanLayer};
use sentry::protocol::{DebugImage, Event};
use sentry::types::Dsn;
use sentry::{ClientInitGuard, ClientOptions, SessionMode};
use tracing::Level;
use tracing_subscriber::filter::{EnvFilter, Targets};
use tracing_subscriber::prelude::*;

/// Reported as the server name, in place of the host name Sentry would send.
const SERVICE_NAME: &str = "hermit";

/// The production DSN, baked in at compile time. Builds without it send nothing
/// unless `SENTRY_DSN` is set. A DSN only allows sending events, but it can be read
/// out of the binary: anyone can send to it.
const BUILT_IN_DSN: Option<&str> = option_env!("HERMIT_SENTRY_DSN");

/// How long quitting waits for the last events to go out.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Stderr filter when `RUST_LOG` is unset.
const DEFAULT_LOG_FILTER: &str = "hermit=info";

/// Keeps the Sentry client running; dropping it flushes and stops it.
#[must_use = "telemetry stops when this is dropped"]
pub struct Telemetry {
    client: Option<ClientInitGuard>,
}

impl Telemetry {
    /// Ends the session, sends pending telemetry and stops sending. Waits at most
    /// [`SHUTDOWN_TIMEOUT`]; later calls do nothing.
    pub fn shutdown(&mut self) {
        let Some(client) = self.client.take() else {
            return;
        };
        // The session lives on this thread's hub, not the one the thread below gets.
        sentry::end_session();

        // Sending is blocking HTTP: run it off this thread so an unreachable server
        // cannot stall quitting.
        let (done, finished) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("telemetry-shutdown".to_owned())
            .spawn(move || {
                drop(client);
                // The receiver is gone only when shutdown already timed out.
                done.send(()).ok();
            });
        match spawned {
            Ok(_) => {
                // A little longer than the client's own timeout, which it applies to
                // the queue and not to a request already in flight.
                if finished
                    .recv_timeout(SHUTDOWN_TIMEOUT + Duration::from_millis(500))
                    .is_err()
                {
                    tracing::warn!(
                        timeout_secs = SHUTDOWN_TIMEOUT.as_secs(),
                        "telemetry did not flush in time"
                    );
                }
            }
            Err(error) => tracing::warn!(%error, "could not flush telemetry"),
        }
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Installs the global `tracing` subscriber and, when a DSN is configured, the
/// Sentry client. Records from the `log` crate (GPUI and other dependencies) are
/// routed through `tracing` too.
///
/// # Panics
///
/// When a global `tracing` subscriber or `log` logger is already installed.
pub fn init() -> Telemetry {
    let env = |name: &str| std::env::var(name).ok();
    let dsn = resolve_dsn(env, BUILT_IN_DSN).map(|(dsn, source)| {
        dsn.parse::<Dsn>()
            .map(|dsn| (dsn, source))
            .with_context(|| format!("{source} is not a valid Sentry DSN"))
    });
    let client = match &dsn {
        Some(Ok((dsn, _))) => Some(sentry::init(client_options(dsn.clone()))),
        Some(Err(_)) | None => None,
    };

    let stderr_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    let sentry_layer = client.as_ref().map(|_| {
        SpanLayer::new()
            .with_filter(own(export_level()))
            .and_then(LogLayer::new().with_filter(own(export_level())))
            .and_then(ErrorLayer::new().with_filter(own(Level::ERROR)))
    });
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(stderr_filter))
        .with(sentry_layer)
        .init();

    match dsn {
        Some(Ok((_, source))) => tracing::info!(dsn = %source, "sending telemetry to Sentry"),
        Some(Err(error)) => tracing::warn!("telemetry is off: {error:#}"),
        None => tracing::debug!("telemetry is off: no Sentry DSN"),
    }
    Telemetry { client }
}

/// Formats `path` with the user's home directory replaced by `~`, so log lines
/// (which are sent) do not carry the user name.
pub fn private_path(path: &Path) -> PrivatePath<'_> {
    PrivatePath {
        path,
        home: std::env::home_dir(),
    }
}

/// A path that displays without the user's home directory; see [`private_path`].
pub struct PrivatePath<'a> {
    path: &'a Path,
    home: Option<PathBuf>,
}

impl fmt::Display for PrivatePath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let relative = self
            .home
            .as_deref()
            .filter(|home| !home.as_os_str().is_empty())
            .and_then(|home| self.path.strip_prefix(home).ok());
        match relative {
            Some(relative) if relative.as_os_str().is_empty() => f.write_str("~"),
            Some(relative) => {
                write!(f, "~{}{}", std::path::MAIN_SEPARATOR, relative.display())
            }
            None => self.path.display().fmt(f),
        }
    }
}

/// Product metrics, sent to Sentry. `tracing` has no metrics API, so these are the
/// one kind of telemetry not recorded through it. They do nothing while telemetry is
/// off.
pub mod metrics {
    use std::time::Duration;

    use sentry::protocol::Unit;

    /// Attributes to filter and group a metric by. Keep them low-cardinality and free
    /// of anything that identifies the user.
    pub type Attributes<'a> = &'a [(&'static str, &'static str)];

    /// Counts one occurrence of `name`.
    pub fn count(name: &'static str, attributes: Attributes<'_>) {
        attributes
            .iter()
            .fold(
                sentry::metrics::counter(name, 1),
                |metric, &(key, value)| metric.attribute(key, value),
            )
            .capture();
    }

    /// Records how long something named `name` took.
    pub fn duration(name: &'static str, duration: Duration, attributes: Attributes<'_>) {
        let milliseconds = duration.as_secs_f64() * 1000.0;
        attributes
            .iter()
            .fold(
                sentry::metrics::distribution(name, milliseconds).unit(Unit::Millisecond),
                |metric, &(key, value)| metric.attribute(key, value),
            )
            .capture();
    }
}

/// Spans and log events at this level and above are sent.
fn export_level() -> Level {
    if cfg!(debug_assertions) {
        Level::DEBUG
    } else {
        Level::INFO
    }
}

/// The share of traces sent: all of them in debug builds, 10% in release builds.
/// Sampling is per trace, so a trace is sent whole or not at all. Errors, logs and
/// metrics are not sampled.
fn traces_sample_rate() -> f32 {
    if cfg!(debug_assertions) { 1.0 } else { 0.1 }
}

/// Hermit's own spans and events at `level` and above. Sending other crates' events
/// would also send the HTTP client's diagnostics about sending them.
fn own(level: Level) -> Targets {
    Targets::new().with_target(env!("CARGO_CRATE_NAME"), level)
}

/// Where the DSN came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DsnSource {
    Environment,
    BuiltIn,
}

impl fmt::Display for DsnSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Environment => "SENTRY_DSN",
            Self::BuiltIn => "the built-in DSN",
        })
    }
}

/// The DSN to send to, or `None` when telemetry is off.
fn resolve_dsn(
    env: impl Fn(&str) -> Option<String>,
    built_in: Option<&'static str>,
) -> Option<(String, DsnSource)> {
    match env("SENTRY_DSN") {
        // Set but empty turns sending off, even in a build with a built-in DSN.
        Some(dsn) => {
            let dsn = dsn.trim();
            (!dsn.is_empty()).then(|| (dsn.to_owned(), DsnSource::Environment))
        }
        None => built_in
            .map(str::trim)
            .filter(|dsn| !dsn.is_empty())
            .map(|dsn| (dsn.to_owned(), DsnSource::BuiltIn)),
    }
}

/// The environment defaults to `development` in debug builds and `production` in
/// release builds; `SENTRY_ENVIRONMENT` overrides it.
fn client_options(dsn: Dsn) -> ClientOptions {
    let mut options = ClientOptions::new()
        .maybe_release(sentry::release_name!())
        .server_name(SERVICE_NAME)
        .send_default_pii(false)
        // One session per run, for crash-free rates per release.
        .auto_session_tracking(true)
        .session_mode(SessionMode::Application)
        .traces_sample_rate(traces_sample_rate())
        .shutdown_timeout(SHUTDOWN_TIMEOUT)
        .before_send({
            let home = std::env::home_dir();
            move |event| Some(scrub_event(event, home.as_deref()))
        });
    options.dsn = Some(dsn);
    options
}

/// Removes the home directory from the paths of the loaded libraries that crash
/// reports list, which would carry the user name when Hermit is installed under it.
fn scrub_event(mut event: Event<'static>, home: Option<&Path>) -> Event<'static> {
    for image in &mut event.debug_meta.to_mut().images {
        if let DebugImage::Symbolic(image) = image {
            image.name = PrivatePath {
                path: Path::new(&image.name),
                home: home.map(Path::to_path_buf),
            }
            .to_string();
        }
    }
    event
}

#[cfg(test)]
mod tests {
    use super::{DsnSource, resolve_dsn};

    const DSN: &str = "https://key@o1.ingest.sentry.io/2";

    fn env_of<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn runtime_dsn_overrides_built_in_dsn() {
        let env = [("SENTRY_DSN", " https://other@example.com/1 ")];
        assert_eq!(
            resolve_dsn(env_of(&env), Some(DSN)),
            Some((
                "https://other@example.com/1".to_owned(),
                DsnSource::Environment
            ))
        );
    }

    #[test]
    fn built_in_dsn_is_used_without_variable() {
        assert_eq!(
            resolve_dsn(env_of(&[]), Some(DSN)),
            Some((DSN.to_owned(), DsnSource::BuiltIn))
        );
    }

    #[test]
    fn empty_runtime_dsn_turns_telemetry_off() {
        let env = [("SENTRY_DSN", "  ")];
        assert_eq!(resolve_dsn(env_of(&env), Some(DSN)), None);
    }

    #[test]
    fn no_dsn_means_off() {
        assert_eq!(resolve_dsn(env_of(&[]), None), None);
        assert_eq!(resolve_dsn(env_of(&[]), Some("")), None);
    }

    /// Unix paths only: on Windows, the part after `~` is joined with `\`.
    #[cfg(unix)]
    mod paths {
        use std::path::{Path, PathBuf};

        use sentry::protocol::{DebugImage, DebugMeta, Event, SymbolicDebugImage};

        use super::super::{PrivatePath, scrub_event};

        #[test]
        fn crash_reports_hide_home_directory() {
            let image = |name: &str| {
                DebugImage::Symbolic(SymbolicDebugImage {
                    name: name.to_owned(),
                    arch: None,
                    image_addr: 0.into(),
                    image_size: 0,
                    image_vmaddr: 0.into(),
                    id: Default::default(),
                    code_id: None,
                    debug_file: None,
                })
            };
            let event = Event {
                debug_meta: std::borrow::Cow::Owned(DebugMeta {
                    images: vec![
                        image("/home/alice/.local/bin/hermit"),
                        image("/usr/lib/libc.so.6"),
                    ],
                    ..DebugMeta::default()
                }),
                ..Event::default()
            };

            let event = scrub_event(event, Some(Path::new("/home/alice")));
            let names: Vec<_> = event
                .debug_meta
                .images
                .iter()
                .filter_map(|image| match image {
                    DebugImage::Symbolic(image) => Some(image.name.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(names, ["~/.local/bin/hermit", "/usr/lib/libc.so.6"]);
        }

        fn private(path: &str, home: &str) -> String {
            PrivatePath {
                path: Path::new(path),
                home: Some(PathBuf::from(home)),
            }
            .to_string()
        }

        #[test]
        fn private_path_hides_home_directory() {
            assert_eq!(
                private("/home/alice/.local/share/applications", "/home/alice"),
                "~/.local/share/applications"
            );
            assert_eq!(private("/home/alice", "/home/alice"), "~");
        }

        #[test]
        fn private_path_keeps_paths_outside_home() {
            assert_eq!(
                private("/run/user/1000/hermit.lock", "/home/alice"),
                "/run/user/1000/hermit.lock"
            );
            assert_eq!(private("/home/alice2/x", "/home/alice"), "/home/alice2/x");
        }
    }
}
