# Hermit

A fast, keyboard-driven launcher for **Linux (Wayland), macOS and Windows**, inspired by [Raycast](https://www.raycast.com/).

Raycast is a great productivity tool, but it doesn't run on Linux. Hermit aims to bring the same experience to every platform: press a hotkey, type a few characters, and get things done without reaching for the mouse.

> **Status:** early development. Hermit runs in the background with a tray icon, and a global hotkey toggles a placeholder window. Everything else listed under [Goals](#goals) is planned, not yet built.

## Goals

- **Instant command palette:** a global hotkey opens a search bar that appears immediately and gets out of the way just as fast.
- **App launcher:** fuzzy-search and launch installed applications (`.desktop` entries on Linux, `.app` bundles on macOS, Start menu entries on Windows).
- **Built-in commands:** calculator, clipboard history, file search, window management, system actions (lock, sleep, restart, etc.).
- **Extensions:** a way to add your own commands and integrations.
- **Native feel on every platform:** one codebase, GPU-accelerated UI, and low memory use.

## Tech stack

- **[Rust](https://www.rust-lang.org/)** (edition 2024)
- **[GPUI](https://www.gpui.rs/)**, the GPU-accelerated UI framework from the Zed editor
- **[gpui-kit](https://github.com/longbridge/gpui-kit)**, a component library on top of GPUI (inputs, lists, dialogs, theming)

## Getting started

### Prerequisites

- A recent stable Rust toolchain (install with [rustup](https://rustup.rs/))
- **Linux:** GPUI's system dependencies (Vulkan drivers, `libxkbcommon`, Wayland development headers). Hermit runs on Wayland only; X11 is not supported. See the [Zed Linux build docs](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md) for the exact package list for your distro.
- **macOS:** Xcode and the Xcode command line tools
- **Windows:** the MSVC toolchain (Visual Studio Build Tools with the "Desktop development with C++" workload)

### Global hotkey

Hermit starts in the background with no window. Press the hotkey to open the launcher, and press it again, hit `Escape`, or click away to close it.

- **Linux:** the shortcut is registered through the XDG desktop portal (`GlobalShortcuts`). Hermit suggests `Super+Space`. Your desktop may ask you to confirm it, and you can change it later in its shortcut settings (on KDE: System Settings → Shortcuts → Hermit). The portal identifies apps by their desktop entry, so Hermit writes `~/.local/share/applications/dev.hermit.Hermit.desktop` on startup.
- **macOS:** `Option+Space`.
- **Windows:** `Alt+Space`. This replaces Windows' own Alt+Space window menu, as PowerToys Run does.

### Tray icon

While Hermit runs, its icon sits in the system tray (the notification area, next to apps like Steam). Left-click it to open the launcher; right-click it for **Open Hermit** and **Quit**. On Linux the tray needs a StatusNotifierItem host: KDE Plasma has one built in, while GNOME needs the AppIndicator extension.

The icon's source is `assets/icon.svg`. After editing it, re-render the PNG the app embeds:

```sh
rsvg-convert -w 64 -h 64 assets/icon.svg -o assets/icon.png
```

Only one Hermit runs at a time: starting it again while it is running just exits.

### Telemetry

Hermit reports traces, logs and metrics with [OpenTelemetry](https://opentelemetry.io/) over OTLP/HTTP: startup and hotkey/tray registration spans, its log lines (linked to those spans), and launcher metrics (`hermit.launcher.opens`, `hermit.launcher.open.duration`, `hermit.launcher.visible.duration`). About the machine it sends only the app version, OS and CPU architecture; file paths in log lines show the home directory as `~`, so user names are not sent.

Each signal (traces, logs, metrics) goes to the first endpoint found for it:

1. `OTEL_EXPORTER_OTLP_{TRACES,LOGS,METRICS}_ENDPOINT` or `OTEL_EXPORTER_OTLP_ENDPOINT` at runtime.
2. `HERMIT_OTLP_ENDPOINT` set at **compile time**, with `HERMIT_OTLP_HEADERS` for its credentials (see [Grafana Cloud](#grafana-cloud) below).
3. Debug builds only: `http://localhost:4318`.

In a release build, a signal with no endpoint is not sent. `OTEL_SDK_DISABLED=true` turns telemetry off. Other standard `OTEL_*` variables (headers, `OTEL_EXPORTER_OTLP_TIMEOUT`, `OTEL_METRIC_EXPORT_INTERVAL`) work too; without a timeout variable each export gives up after 5 seconds.

`RUST_LOG` controls what is printed to stderr; it does not change what is exported. By default that is `hermit=info,opentelemetry=warn`, so export failures show up. When a debug build only sends to the implicit `localhost:4318`, export failures are hidden (`opentelemetry=off`), so `cargo run` stays quiet without a collector.

For local development, start a collector and [Jaeger](https://www.jaegertracing.io/):

```sh
docker compose up -d
cargo run
```

Traces appear in the Jaeger UI at <http://localhost:16686> (service `hermit`). Jaeger stores traces only, so the collector prints logs and metrics instead: `docker compose logs -f otel-collector`. Metrics export every 60 seconds and on quit; run with `OTEL_METRIC_EXPORT_INTERVAL=5000` to see them sooner.

#### Grafana Cloud

Release builds send telemetry straight to Grafana Cloud's OTLP endpoint. Get the endpoint and instance ID from your stack's **OpenTelemetry** tile in the Grafana Cloud portal. Then create a token under **Administration → Users and access → Cloud access policies**, with a policy that only has `metrics:write`, `logs:write` and `traces:write` on that one stack.

```sh
HERMIT_OTLP_ENDPOINT=https://otlp-gateway-prod-<region>.grafana.net/otlp \
HERMIT_OTLP_HEADERS="Authorization=Basic%20$(printf '%s' '<instance id>:<token>' | base64 -w0)" \
cargo build --release
```

The token is compiled into the binary, and anyone with a copy of Hermit can extract it. That is why it may only write telemetry: it cannot read data or change the account, but it could be used to send junk data into the stack. If that happens, revoke it and build with a new one. The built-in headers are sent only to the built-in endpoint, never to one set with `OTEL_EXPORTER_OTLP_*`.

To try Grafana Cloud from a debug build, set the same values at runtime instead, as `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_EXPORTER_OTLP_HEADERS`. The easiest place is a `.env` file: debug builds load it at startup from the working directory or a parent (release builds never do). Copy [`.env.example`](.env.example) to `.env`, which git ignores, and fill it in. Variables already set in your shell take precedence, and while `.env` sets an endpoint, `cargo run` sends to Grafana Cloud instead of the local collector. Data shows up in Grafana under service `hermit`: traces in Tempo, logs in Loki, metrics in Mimir.

### Build and run

```sh
git clone <repo-url> hermit
cd hermit
cargo run
```

For an optimized build:

```sh
cargo build --release
./target/release/hermit
```

## Project structure

```
src/
  main.rs        # Application entry point
  launcher.rs    # The launcher window, toggled by the hotkey
  tray/          # Tray icon and menu: ksni (Linux), tray-icon crate (macOS, Windows)
  icon.rs        # Decodes the embedded app icon (assets/icon.png)
  single_instance.rs  # Lock file that keeps a second Hermit from starting
  telemetry.rs   # tracing subscriber and OpenTelemetry (OTLP) exporters
assets/
  icon.svg       # App and tray icon (source)
  icon.png       # 64px render of icon.svg, embedded in the binary
  hotkey/        # Global hotkey: XDG portal (Wayland), global-hotkey crate (macOS, Windows)
dev/
  otel-collector.yaml  # Collector config for the local telemetry stack
compose.yaml     # Local OpenTelemetry Collector + Jaeger
.agents/skills/  # Agent skills for AI coding assistants (gpui-kit guides)
```

## Contributing

The project is still taking shape, so ideas, issues, and pull requests are welcome. Before submitting code, please run:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
