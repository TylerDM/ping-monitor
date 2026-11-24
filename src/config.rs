use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const MIN_INTERVAL_MS: u32 = 250;
pub const MAX_INTERVAL_MS: u32 = 3_600_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Host name or IPv4 address to ping.
    pub host: String,
    /// Time between pings, in milliseconds.
    pub interval_ms: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: "1.1.1.1".into(),
            interval_ms: 5_000,
        }
    }
}

/// `%APPDATA%\PingMonitor\config.json` (per user).
pub fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("PingMonitor")
            .join("config.json"),
    )
}

/// Loads the saved config, falling back to defaults for a missing or unreadable file.
pub fn load() -> Config {
    let mut config = path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
        .unwrap_or_default();

    config.host = config.host.trim().to_string();
    if config.host.is_empty() {
        config.host = Config::default().host;
    }
    config.interval_ms = config.interval_ms.clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
    config
}

pub fn save(config: &Config) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("APPDATA is not set"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(config)?;
    // Write to a temp file first so a crash mid-write can't leave a truncated config.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}
