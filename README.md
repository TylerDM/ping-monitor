# Ping Monitor

A tiny Windows tray app whose icon is your current ping, in milliseconds, to a host you choose.

- **Left-click** the icon to open the Stats tab: a chart of latency and packet loss over the last 12 hours (kept in memory only, so it starts empty each time the app starts), with a summary (now, average, min, max, loss). It updates live.
- The **Settings** tab sets the host, ping interval, and the icon's font family, weight and size.
- **Right-click** for a menu with Open, Settings, Check for updates and Quit.
- **Hover** for a tooltip with the last result, plus the average and packet loss over the last 60 pings.

The number is drawn in the taskbar's own font (Segoe UI Variable) and text colour up to 50 ms, then shifts towards red, fully red at 200 ms. `×` means the request timed out and `!` means an error (hover for details). Colours follow the light or dark taskbar theme.

## Installing

Download **[PingMonitor.msi](https://github.com/TylerDM/ping-monitor/releases/latest/download/PingMonitor.msi)** from the latest release and run it. It installs for your account only (no admin prompt) into `%LOCALAPPDATA%\Programs\PingMonitor`, adds a Start Menu shortcut, starts Ping Monitor at sign-in, and launches it.

The MSI isn't code-signed, so the first time, SmartScreen may say "Windows protected your PC": choose **More info**, then **Run anyway**.

- **Updates:** Ping Monitor checks GitHub for a new release a minute after it starts and then daily. When there is one, a notification offers it; click it (or **Install update** in the tray menu) and it downloads, installs and restarts itself. **Check for updates** in the menu checks right away. Installing a newer MSI by hand works too.
- **Starting at sign-in:** switch it off in Task Manager → Startup apps.
- **Uninstalling:** Settings → Apps → Installed apps → Ping Monitor → Uninstall. Your settings in `%APPDATA%\PingMonitor` are kept.

## Settings

Settings are stored per user in `%APPDATA%\PingMonitor\config.json`:

```json
{
  "host": "1.1.1.1",
  "interval_ms": 5000,
  "font_family": "Segoe UI Variable Text",
  "font_weight": 400,
  "font_size_pt": null
}
```

The interval can be anything from 250 ms to 1 hour. `font_size_pt` is in points, or `null` to fit the text to the icon. Changes made on the Settings tab take effect immediately.

## Building

You need Rust (MSVC toolchain) and the Visual Studio Build Tools with the C++ workload.

```bash
cargo build --release
```

The binary ends up at `target\release\ping-monitor.exe`. Release builds have no console window; debug builds keep one for logging.

The installer is built with [WiX](https://wixtoolset.org/), pinned as a repo-local dotnet tool in `dotnet-tools.json` (it needs the .NET SDK). `scripts/build-msi.ps1` builds `target\wix\PingMonitor.msi`, versioned from `Cargo.toml`.

To build and install your working copy the same way a release installs, replacing whatever version is installed:

```bash
pwsh -File install.ps1
```

`install.ps1 -Uninstall` removes it.

## Releasing

```bash
pwsh -File scripts/release.ps1 0.2.0
```

That bumps the version in `Cargo.toml`, commits, tags `v0.2.0` and pushes. The tag starts `.github/workflows/release.yml`, which builds the MSI and publishes the GitHub release; installed copies offer the update within a day. Versions must go up: the updater only offers a release newer than what's running, and the MSI refuses to downgrade.

## Cross-platform development

The app only runs on Windows, but the repository works the same from Linux. The goal: a file has the same bytes on Windows and Linux, and nobody ever sees a diff that is purely line endings.

**`.gitattributes`** is committed, so it applies to every clone. It sets `* text=auto eol=lf`, which **overrides** `core.autocrlf`, so it works no matter how a given machine is configured. The exceptions are `.bat` and `.cmd`, which keep CRLF because the Windows batch interpreter requires it.

**`scripts/setup-git.ts`** applies what `.gitattributes` cannot, because those settings live in `.git/config` and are not cloned. Run it once after cloning:

```bash
node scripts/setup-git.ts
```

| Setting                              | Why                                                   |
| ------------------------------------ | ----------------------------------------------------- |
| `core.autocrlf=false`, `core.eol=lf` | Let `.gitattributes` be the only authority            |
| `core.ignorecase=false`              | `Foo.rs` and `foo.rs` are different files             |
| `core.longpaths=true`                | `target/` paths can exceed the 260-char Windows limit |
| `merge.conflictstyle=zdiff3`         | Conflict markers include the common ancestor          |

`.editorconfig` covers the other half: what your editor writes. Both agree on LF, UTF-8, and a final newline.

**`rust-toolchain.toml`** pins the compiler and adds the `x86_64-pc-windows-msvc` target everywhere. On Linux you can't link the app, but you can check and lint it as Windows code:

```bash
cargo clippy --target x86_64-pc-windows-msvc
```

The VS Code settings point rust-analyzer at that target too, so it resolves the Win32 imports.

> **Case sensitivity.** `core.ignorecase=false` assumes the source directory is case-sensitive. On Windows that is a per-directory flag. Check and enable it:
>
> ```bash
> fsutil.exe file queryCaseSensitiveInfo X:\source
> ```
>
> If the directory is _not_ case-sensitive, drop `core.ignorecase` from `scripts/setup-git.ts`; otherwise git reports phantom renames.

If the whole repo ever shows as modified, it is a normalization mismatch:

```bash
git add --renormalize .
```

## Notes

- Pings use the Windows ICMP API (`IcmpSendEcho`), so no administrator rights are needed.
- Only IPv4 is supported for now. Host names resolve to their first IPv4 address.
