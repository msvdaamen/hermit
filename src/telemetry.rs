//! Logging and OpenTelemetry: traces, logs and metrics exported over OTLP/HTTP.
//!
//! Hermit always prints its own log lines to stderr, filtered by `RUST_LOG`. It also
//! exports each signal (traces, logs, metrics) to the first endpoint it finds for
//! that signal:
//!
//! 1. `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT` or `OTEL_EXPORTER_OTLP_ENDPOINT` in the
//!    environment at runtime;
//! 2. `HERMIT_OTLP_ENDPOINT`, set in the environment when Hermit is compiled;
//! 3. in debug builds only, the local collector at `http://localhost:4318`
//!    (`docker compose up` starts one, with Jaeger behind it).
//!
//! In a release build, a signal with none of these is not exported. `OTEL_SDK_DISABLED=true`
//! turns export off in any build.
//!
//! Exported data must not identify the user: log file system paths through
//! [`private_path`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::Level;
use tracing_subscriber::filter::{EnvFilter, Targets};
use tracing_subscriber::prelude::*;

/// The `service.name` every signal is reported under.
const SERVICE_NAME: &str = "hermit";

/// Base URL of the production collector, baked in at compile time. Unset for now:
/// release builds export nothing until a backend is chosen.
const BUILT_IN_ENDPOINT: Option<&str> = option_env!("HERMIT_OTLP_ENDPOINT");

/// Where debug builds send telemetry when nothing else is configured.
const LOCAL_COLLECTOR: &str = "http://localhost:4318";

/// Per request, unless an `OTEL_EXPORTER_OTLP_*TIMEOUT` variable says otherwise.
/// Short, so a dead collector cannot hold a batch for long.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long quitting waits for the last batches to go out.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Stderr filter when `RUST_LOG` is unset. The exporters report failures under the
/// `opentelemetry*` targets.
const DEFAULT_LOG_FILTER: &str = "hermit=info,opentelemetry=warn";

/// Stderr filter when `RUST_LOG` is unset and telemetry only goes to the implicit
/// local collector. Without one running, every batch would print an export failure.
const LOCAL_LOG_FILTER: &str = "hermit=info,opentelemetry=off";

/// Keeps the exporters running; dropping it flushes and stops them.
#[must_use = "telemetry stops when this is dropped"]
pub struct Telemetry {
    providers: Option<Providers>,
}

impl Telemetry {
    /// Flushes pending telemetry and stops exporting. Waits at most
    /// [`SHUTDOWN_TIMEOUT`]; later calls do nothing.
    pub fn shutdown(&mut self) {
        let Some(providers) = self.providers.take() else {
            return;
        };

        // Exporting is blocking HTTP: run it off this thread so an unreachable
        // endpoint cannot stall quitting.
        let (done, finished) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("telemetry-shutdown".to_owned())
            .spawn(move || {
                providers.shutdown();
                // The receiver is gone only when shutdown already timed out.
                done.send(()).ok();
            });
        match spawned {
            Ok(_) => {
                if finished.recv_timeout(SHUTDOWN_TIMEOUT).is_err() {
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

/// Installs the global `tracing` subscriber and, when an endpoint is configured,
/// the OpenTelemetry exporters. Records from the `log` crate (GPUI and other
/// dependencies) are routed through `tracing` too.
///
/// # Panics
///
/// When a global `tracing` subscriber or `log` logger is already installed.
pub fn init() -> Telemetry {
    let env = |name: &str| std::env::var(name).ok();
    let endpoints = Endpoints::resolve(env, BUILT_IN_ENDPOINT, cfg!(debug_assertions));
    let providers = if endpoints.is_off() {
        Ok(None)
    } else {
        Providers::new(&endpoints, env).map(Some)
    };

    let default_filter = if endpoints.is_local_only() {
        LOCAL_LOG_FILTER
    } else {
        DEFAULT_LOG_FILTER
    };
    let stderr_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let export_layer = providers
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(|providers| {
            tracing_opentelemetry::layer()
                .with_tracer(providers.tracer.tracer(SERVICE_NAME))
                .and_then(OpenTelemetryTracingBridge::new(&providers.logger))
                // Only Hermit's own spans and events. Exporting the exporters' own
                // diagnostics (HTTP client, OpenTelemetry) would feed back into itself.
                .with_filter(Targets::new().with_target(env!("CARGO_CRATE_NAME"), export_level()))
        });
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(stderr_filter))
        .with(export_layer)
        .init();

    let providers = match providers {
        Ok(Some(providers)) => {
            opentelemetry::global::set_meter_provider(providers.meter.clone());
            tracing::info!(
                traces = %endpoints.traces,
                logs = %endpoints.logs,
                metrics = %endpoints.metrics,
                "exporting telemetry"
            );
            Some(providers)
        }
        Ok(None) => {
            tracing::debug!("telemetry export is off");
            None
        }
        Err(error) => {
            tracing::warn!("could not start telemetry export: {error:#}");
            None
        }
    };
    Telemetry { providers }
}

/// Formats `path` with the user's home directory replaced by `~`, so log lines
/// (which are exported) do not carry the user name.
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

/// Spans and log events at this level and above are exported.
fn export_level() -> Level {
    if cfg!(debug_assertions) {
        Level::DEBUG
    } else {
        Level::INFO
    }
}

/// An OpenTelemetry signal, each with its own exporter and configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Signal {
    Traces,
    Logs,
    Metrics,
}

impl Signal {
    /// The lowercase name used in OTLP paths (`/v1/traces`).
    fn name(self) -> &'static str {
        match self {
            Self::Traces => "traces",
            Self::Logs => "logs",
            Self::Metrics => "metrics",
        }
    }

    /// This signal's variant of an exporter variable, such as
    /// `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` for `setting` `ENDPOINT`.
    fn variable(self, setting: &str) -> String {
        format!(
            "OTEL_EXPORTER_OTLP_{}_{setting}",
            self.name().to_ascii_uppercase()
        )
    }
}

/// Where one signal goes.
#[derive(Debug, PartialEq, Eq)]
enum Endpoint {
    /// Configured through `OTEL_EXPORTER_OTLP_*` variables, which the exporter reads.
    Environment,
    /// The compile-time base URL.
    BuiltIn(&'static str),
    /// Debug builds' fallback, [`LOCAL_COLLECTOR`].
    Local,
    /// Not exported.
    Off,
}

impl Endpoint {
    fn resolve(
        signal: Signal,
        env: impl Fn(&str) -> Option<String>,
        built_in: Option<&'static str>,
        is_debug_build: bool,
    ) -> Self {
        let is_set = |name: &str| env(name).is_some_and(|value| !value.trim().is_empty());

        if env("OTEL_SDK_DISABLED").is_some_and(|value| value.trim().eq_ignore_ascii_case("true")) {
            return Self::Off;
        }
        if is_set(&signal.variable("ENDPOINT")) || is_set("OTEL_EXPORTER_OTLP_ENDPOINT") {
            return Self::Environment;
        }
        match built_in.filter(|base| !base.trim().is_empty()) {
            Some(base) => Self::BuiltIn(base),
            None if is_debug_build => Self::Local,
            None => Self::Off,
        }
    }

    /// The URL to set on the exporter, or `None` when it reads its own configuration.
    fn url(&self, signal: Signal) -> Option<String> {
        match self {
            Self::BuiltIn(base) => Some(signal_url(base, signal)),
            Self::Local => Some(signal_url(LOCAL_COLLECTOR, signal)),
            Self::Environment | Self::Off => None,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment => f.write_str("OTEL_EXPORTER_OTLP_*"),
            Self::BuiltIn(base) => f.write_str(base),
            Self::Local => f.write_str(LOCAL_COLLECTOR),
            Self::Off => f.write_str("off"),
        }
    }
}

/// Each signal's endpoint.
#[derive(Debug, PartialEq, Eq)]
struct Endpoints {
    traces: Endpoint,
    logs: Endpoint,
    metrics: Endpoint,
}

impl Endpoints {
    fn resolve(
        env: impl Fn(&str) -> Option<String>,
        built_in: Option<&'static str>,
        is_debug_build: bool,
    ) -> Self {
        let resolve = |signal| Endpoint::resolve(signal, &env, built_in, is_debug_build);
        Self {
            traces: resolve(Signal::Traces),
            logs: resolve(Signal::Logs),
            metrics: resolve(Signal::Metrics),
        }
    }

    fn all(&self) -> [&Endpoint; 3] {
        [&self.traces, &self.logs, &self.metrics]
    }

    fn is_off(&self) -> bool {
        self.all()
            .iter()
            .all(|endpoint| **endpoint == Endpoint::Off)
    }

    /// Whether every exported signal goes to the implicit local collector.
    fn is_local_only(&self) -> bool {
        self.all()
            .iter()
            .all(|endpoint| matches!(endpoint, Endpoint::Local | Endpoint::Off))
    }
}

/// The OTLP/HTTP URL for `signal` under `base`.
fn signal_url(base: &str, signal: Signal) -> String {
    format!("{}/v1/{}", base.trim_end_matches('/'), signal.name())
}

/// Whether the environment sets the export timeout for `signal`; a timeout set in
/// code would override it.
fn has_timeout_variable(signal: Signal, env: impl Fn(&str) -> Option<String>) -> bool {
    [
        signal.variable("TIMEOUT"),
        "OTEL_EXPORTER_OTLP_TIMEOUT".to_owned(),
    ]
    .iter()
    .any(|name| env(name).is_some_and(|value| !value.trim().is_empty()))
}

/// Applies the shared settings to a signal's exporter builder, or returns `None`
/// when the signal is not exported.
fn configure<B: WithExportConfig>(
    builder: B,
    signal: Signal,
    endpoint: &Endpoint,
    env: impl Fn(&str) -> Option<String>,
) -> Option<B> {
    if *endpoint == Endpoint::Off {
        return None;
    }
    let builder = if has_timeout_variable(signal, env) {
        builder
    } else {
        builder.with_timeout(EXPORT_TIMEOUT)
    };
    Some(match endpoint.url(signal) {
        Some(url) => builder.with_endpoint(url),
        None => builder,
    })
}

/// The SDK pipelines. Each batches on its own background thread, so none needs
/// an async runtime. A signal that is not exported keeps a provider without an
/// exporter, which drops its data.
struct Providers {
    tracer: SdkTracerProvider,
    logger: SdkLoggerProvider,
    meter: SdkMeterProvider,
}

impl Providers {
    fn new(
        endpoints: &Endpoints,
        env: impl Fn(&str) -> Option<String> + Copy,
    ) -> anyhow::Result<Self> {
        let resource = resource();

        let mut tracer = SdkTracerProvider::builder().with_resource(resource.clone());
        if let Some(builder) = configure(
            SpanExporter::builder().with_http(),
            Signal::Traces,
            &endpoints.traces,
            env,
        ) {
            let exporter = builder.build().context("creating the span exporter")?;
            tracer = tracer.with_batch_exporter(exporter);
        }

        let mut logger = SdkLoggerProvider::builder().with_resource(resource.clone());
        if let Some(builder) = configure(
            LogExporter::builder().with_http(),
            Signal::Logs,
            &endpoints.logs,
            env,
        ) {
            let exporter = builder.build().context("creating the log exporter")?;
            logger = logger.with_batch_exporter(exporter);
        }

        let mut meter = SdkMeterProvider::builder().with_resource(resource);
        if let Some(builder) = configure(
            MetricExporter::builder().with_http(),
            Signal::Metrics,
            &endpoints.metrics,
            env,
        ) {
            let exporter = builder.build().context("creating the metric exporter")?;
            // Exports every 60 s, or per `OTEL_METRIC_EXPORT_INTERVAL` (milliseconds).
            meter = meter.with_periodic_exporter(exporter);
        }

        Ok(Self {
            tracer: tracer.build(),
            logger: logger.build(),
            meter: meter.build(),
        })
    }

    fn shutdown(self) {
        // Spans first: log records and metrics may refer to them.
        if let Err(error) = self.tracer.shutdown() {
            tracing::warn!(%error, "could not flush spans");
        }
        if let Err(error) = self.logger.shutdown() {
            tracing::warn!(%error, "could not flush log records");
        }
        if let Err(error) = self.meter.shutdown() {
            tracing::warn!(%error, "could not flush metrics");
        }
    }
}

/// Describes this Hermit build. Deliberately leaves out anything that identifies
/// the user or the machine (host name, user name, paths).
fn resource() -> Resource {
    Resource::builder()
        .with_service_name(SERVICE_NAME)
        .with_attributes([
            KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
            KeyValue::new(
                "deployment.environment.name",
                if cfg!(debug_assertions) {
                    "development"
                } else {
                    "production"
                },
            ),
            KeyValue::new("os.type", os_type()),
            KeyValue::new("host.arch", host_arch()),
        ])
        .build()
}

/// `os.type` as the semantic conventions spell it.
fn os_type() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// `host.arch` as the semantic conventions spell it.
fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Endpoint, Endpoints, PrivatePath, Signal, has_timeout_variable, signal_url};

    fn env_of<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + Copy + 'a {
        |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn runtime_variable_overrides_built_in_endpoint() {
        let env = [("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318")];
        let endpoints = Endpoints::resolve(env_of(&env), Some("https://otel.example.com"), false);
        assert_eq!(endpoints.traces, Endpoint::Environment);
        assert_eq!(endpoints.metrics, Endpoint::Environment);
    }

    #[test]
    fn per_signal_variable_configures_only_that_signal() {
        let env = [(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://jaeger:4318/v1/traces",
        )];
        let endpoints = Endpoints::resolve(env_of(&env), None, false);
        assert_eq!(
            endpoints,
            Endpoints {
                traces: Endpoint::Environment,
                logs: Endpoint::Off,
                metrics: Endpoint::Off,
            }
        );
    }

    #[test]
    fn built_in_endpoint_is_used_without_variables() {
        let endpoints = Endpoints::resolve(env_of(&[]), Some("https://otel.example.com"), false);
        assert_eq!(
            endpoints.logs,
            Endpoint::BuiltIn("https://otel.example.com")
        );
    }

    #[test]
    fn debug_build_falls_back_to_local_collector() {
        let endpoints = Endpoints::resolve(env_of(&[]), None, true);
        assert_eq!(endpoints.traces, Endpoint::Local);
        assert!(endpoints.is_local_only());
    }

    #[test]
    fn release_build_without_endpoint_exports_nothing() {
        let env = [("OTEL_EXPORTER_OTLP_ENDPOINT", "  ")];
        assert!(Endpoints::resolve(env_of(&env), Some(""), false).is_off());
    }

    #[test]
    fn sdk_disabled_wins() {
        let env = [
            ("OTEL_SDK_DISABLED", "TRUE"),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://collector:4318"),
        ];
        assert!(Endpoints::resolve(env_of(&env), None, true).is_off());
    }

    #[test]
    fn signal_url_appends_path_once() {
        assert_eq!(
            signal_url("https://otel.example.com/", Signal::Traces),
            "https://otel.example.com/v1/traces"
        );
    }

    #[test]
    fn timeout_variables_are_detected_per_signal() {
        let env = [("OTEL_EXPORTER_OTLP_LOGS_TIMEOUT", "30000")];
        assert!(has_timeout_variable(Signal::Logs, env_of(&env)));
        assert!(!has_timeout_variable(Signal::Traces, env_of(&env)));
    }

    fn private(path: &str, home: &str) -> String {
        PrivatePath {
            path: Path::new(path),
            home: Some(PathBuf::from(home)),
        }
        .to_string()
    }

    #[cfg(unix)]
    #[test]
    fn private_path_hides_home_directory() {
        assert_eq!(
            private("/home/alice/.local/share/applications", "/home/alice"),
            "~/.local/share/applications"
        );
        assert_eq!(private("/home/alice", "/home/alice"), "~");
    }

    #[cfg(unix)]
    #[test]
    fn private_path_keeps_paths_outside_home() {
        assert_eq!(
            private("/run/user/1000/hermit.lock", "/home/alice"),
            "/run/user/1000/hermit.lock"
        );
        assert_eq!(private("/home/alice2/x", "/home/alice"), "/home/alice2/x");
    }
}
