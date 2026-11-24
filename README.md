# Ping Monitor

A tiny Windows tray app whose icon is your current ping, in milliseconds, to a host you choose.

- **Left-click** the icon to open the Stats tab: a chart of latency and packet loss over the last 12 hours (kept in memory only, so it starts empty each time the app starts), with a summary (now, average, min, max, loss). It updates live.
- The **Settings** tab sets the host, ping interval, and the icon's font family, weight and size.
- **Right-click** for a menu with Open, Settings and Quit.
- **Hover** for a tooltip with the last result, plus the average and packet loss over the last 60 pings.

The number is drawn in the taskbar's own font (Segoe UI Variable) and text colour up to 50 ms, then shifts towards red, fully red at 200 ms. `×` means the request timed out and `!` means an error (hover for details). Colours follow the light or dark taskbar theme.

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

## Installing

`install.ps1` builds a release, copies it to `%LOCALAPPDATA%\Programs\PingMonitor`, adds a Start Menu shortcut and starts the app. It needs no admin rights. Run it again to update.

```bash
pwsh -File install.ps1 -Startup
```

Leave out `-Startup` if you don't want it starting when you sign in. Run with `-Uninstall` to remove it; your settings are kept.

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
