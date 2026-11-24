# AGENTS.md

Guidance for AI coding agents working in this repository. This is the
cross-tool convention (Claude Code, and others), so there is deliberately no
separate CLAUDE.md to keep in sync.

## What this is

A Windows tray app, written in Rust directly against Win32 (the `windows`
crate), whose icon shows the current ping to a configurable host. There is no
GUI framework: the tray icon is `Shell_NotifyIconW`, and the window it opens is
plain Win32 controls plus a custom-drawn chart.

| File              | What it holds                                                    |
| ----------------- | ---------------------------------------------------------------- |
| `src/main.rs`     | entry point, hidden message window, tray icon, menu, app state   |
| `src/window.rs`   | the window the tray icon opens: Stats and Settings tabs          |
| `src/chart.rs`    | the Stats tab's latency / packet-loss chart (GDI + GDI+)         |
| `src/history.rs`  | 12 hours of timestamped results, summaries, chart downsampling |
| `src/ping.rs`     | ICMP echo (`IcmpSendEcho`) and the background ping thread        |
| `src/icon.rs`     | renders the ping as text into an HICON; taskbar theme detection  |
| `src/fonts.rs`    | finds installed fonts by family through DirectWrite              |
| `src/config.rs`   | the per-user JSON config in `%APPDATA%\PingMonitor\config.json`  |
| `src/update.rs`   | daily GitHub release check, MSI download and install             |
| `src/http.rs`     | minimal HTTPS GET over WinHTTP                                   |
| `installer/`      | the per-user WiX MSI; `scripts/build-msi.ps1` builds it          |
| `app.manifest`    | embedded by `build.rs`: common controls v6, per-monitor DPI      |

## Rules

- **App state lives on the UI thread** in a `thread_local` `RefCell`, reached
  through `with_app`. Message boxes, menus and window creation pump messages
  and re-enter the window procedures, so never hold the borrow across one of
  them. `with_app` uses `try_borrow_mut` and skips work rather than panicking.
- **The ping thread never touches UI state.** It sends results over a channel
  and posts `WM_APP_PING` to wake the UI thread.
- **Windows is the only target.** On Linux, check with
  `cargo clippy --target x86_64-pc-windows-msvc`. It never links, so no Windows
  SDK is needed.
- **Never change the MSI's `UpgradeCode`** in `installer/PingMonitor.wxs`. It
  is how a new version finds and replaces the installed one.
- **Releases come from tags.** `scripts/release.ps1 X.Y.Z` bumps `Cargo.toml`,
  tags and pushes; the release workflow refuses a tag that doesn't match. The
  asset must stay named `PingMonitor.msi`: the updater looks for that name.
- **Line endings are LF everywhere** except `.bat`/`.cmd`. See
  `.gitattributes`.

## Before finishing a change

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo build
```
