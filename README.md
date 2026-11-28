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

Hermit reports to [Sentry](https://sentry.io/): crashes (panics), errors, traces (startup, hotkey and tray registration, opening the launcher), its log lines (linked to those traces), release-health sessions, and launcher metrics (`hermit.launcher.opens`, `hermit.launcher.open.duration`, `hermit.launcher.visible.duration`). Code records spans and events with [`tracing`](https://docs.rs/tracing); `src/telemetry.rs` routes them to Sentry, and is the only place that uses the Sentry SDK. Metrics go through `telemetry::metrics`, because `tracing` has no metrics API.

About the machine, Hermit sends the app version, OS and version, and CPU architecture; it does not send the host name or IP address. File paths in log lines and crash reports show the home directory as `~`, so user names are not sent.

Hermit sends to the first DSN it finds:

1. `SENTRY_DSN` at runtime. Set but empty, it turns telemetry off, even in a build with a built-in DSN.
2. `HERMIT_SENTRY_DSN` set at **compile time**:

   ```sh
   HERMIT_SENTRY_DSN=https://<key>@<org>.ingest.<region>.sentry.io/<project> cargo build --release
   ```

Without either, nothing is sent. Find the DSN in Sentry under **Settings → Projects → hermit → Client Keys (DSN)**. A DSN can only send events, but anyone with a copy of Hermit can read it out of the binary and send junk to the project. Sentry's per-key rate limits (on the same page) cap the damage; if it gets abused, revoke the key and build with a new one.

Events are tagged with environment `development` in debug builds and `production` in release builds (`SENTRY_ENVIRONMENT` overrides it), and with release `hermit@<version>`. Spans and logs from Hermit's own code are sent at `debug` and above in debug builds, `info` and above in release builds; `error!` events also open Sentry issues. Debug builds send every trace, release builds a random 10% (sampled per trace, so a trace arrives whole or not at all). Errors, crashes, logs and metrics are always sent in full.

`RUST_LOG` controls what is printed to stderr (default `hermit=info`); it does not change what is sent.

To send from a debug build, put a DSN in `.env`: debug builds load it at startup from the working directory or a parent (release builds never do). Copy [`.env.example`](.env.example) to `.env`, which git ignores, and fill it in. Variables already set in your shell take precedence. Use a separate Sentry project for development, or filter by the `development` environment.

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
  hotkey/        # Global hotkey: XDG portal (Wayland), global-hotkey crate (macOS, Windows)
  telemetry.rs   # tracing subscriber and Sentry
assets/
  icon.svg       # App and tray icon (source)
  icon.png       # 64px render of icon.svg, embedded in the binary
.agents/skills/  # Agent skills for AI coding assistants (gpui-kit guides)
```

## Contributing

The project is still taking shape, so ideas, issues, and pull requests are welcome. Before submitting code, please run:

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
