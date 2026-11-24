//! Checks GitHub for a newer release and installs its MSI.

use crate::http;
use serde::Deserialize;
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const LATEST_RELEASE: &str = "https://api.github.com/repos/TylerDM/ping-monitor/releases/latest";
/// The release workflow always names the installer this.
const ASSET_NAME: &str = "PingMonitor.msi";
/// Wait after startup before the first check, so it doesn't compete with sign-in.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Without the leading "v", e.g. "0.2.0".
    pub version: String,
    pub msi_url: String,
}

pub enum Event {
    /// A check finished. `manual` is true when the user asked for it.
    Checked {
        manual: bool,
        result: Result<Option<Release>, String>,
    },
    /// The MSI for a release has been downloaded.
    Downloaded(Result<PathBuf, String>),
}

fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.trim_start_matches('v').splitn(3, '.');
    let mut next = || parts.next()?.parse().ok();
    Some((next()?, next()?, next()?))
}

/// The latest release, if it's newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(Deserialize)]
    struct GitHubRelease {
        tag_name: String,
        assets: Vec<Asset>,
    }

    let (status, body) = http::get(LATEST_RELEASE)?;
    match status {
        200 => {}
        // No releases published yet.
        404 => return Ok(None),
        _ => return Err(format!("GitHub returned HTTP {status}")),
    }
    let release: GitHubRelease =
        serde_json::from_slice(&body).map_err(|e| format!("Unexpected reply from GitHub: {e}"))?;
    let (Some(latest), Some(current)) = (
        parse_version(&release.tag_name),
        parse_version(CURRENT_VERSION),
    ) else {
        return Ok(None);
    };
    if latest <= current {
        return Ok(None);
    }
    let msi = release
        .assets
        .into_iter()
        .find(|asset| asset.name.eq_ignore_ascii_case(ASSET_NAME))
        .ok_or_else(|| format!("Release {} has no {ASSET_NAME}", release.tag_name))?;
    Ok(Some(Release {
        version: release.tag_name.trim_start_matches('v').to_string(),
        msi_url: msi.browser_download_url,
    }))
}

/// Downloads the release's MSI into the temp folder.
pub fn download(release: &Release) -> Result<PathBuf, String> {
    let (status, body) = http::get(&release.msi_url)?;
    if status != 200 {
        return Err(format!("Downloading the installer returned HTTP {status}"));
    }
    let path = std::env::temp_dir().join(format!("PingMonitor-{}.msi", release.version));
    std::fs::write(&path, body).map_err(|e| format!("Couldn't save the installer: {e}"))?;
    Ok(path)
}

/// Starts the installer. Its progress bar is all the user sees; when it's
/// done it starts the new version (see installer/PingMonitor.wxs).
pub fn install(msi: &PathBuf) -> std::io::Result<()> {
    std::process::Command::new("msiexec.exe")
        .arg("/i")
        .arg(msi)
        .args(["/passive", "/norestart"])
        .spawn()
        .map(|_| ())
}

fn notify(events: &Sender<Event>, event: Event, hwnd: usize, msg: u32) -> bool {
    if events.send(event).is_err() {
        return false;
    }
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), msg, WPARAM(0), LPARAM(0));
    }
    true
}

/// Runs `job` on a background thread and delivers its event like the others.
fn spawn(
    events: Sender<Event>,
    notify_hwnd: HWND,
    msg: u32,
    job: impl FnOnce() -> Event + Send + 'static,
) {
    let hwnd = notify_hwnd.0 as usize;
    let _ = std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            notify(&events, job(), hwnd, msg);
        });
}

/// Checks shortly after startup and then daily, in the background.
pub fn spawn_checker(events: Sender<Event>, notify_hwnd: HWND, msg: u32) {
    let hwnd = notify_hwnd.0 as usize;
    let _ = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK_DELAY);
            loop {
                let event = Event::Checked {
                    manual: false,
                    result: check(),
                };
                if !notify(&events, event, hwnd, msg) {
                    return;
                }
                std::thread::sleep(CHECK_EVERY);
            }
        });
}

/// A check the user asked for from the menu.
pub fn spawn_manual_check(events: Sender<Event>, notify_hwnd: HWND, msg: u32) {
    spawn(events, notify_hwnd, msg, || Event::Checked {
        manual: true,
        result: check(),
    });
}

pub fn spawn_download(events: Sender<Event>, notify_hwnd: HWND, msg: u32, release: Release) {
    spawn(events, notify_hwnd, msg, move || {
        Event::Downloaded(download(&release))
    });
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert!(parse_version("v0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("latest"), None);
    }
}
