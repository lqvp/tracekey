use anyhow::Result;
use config::{Config, File};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ReportingSettings {
    pub enabled: bool,
    pub interval: String,
    pub output_to_console: bool,
    pub output_to_misskey: bool,
    pub misskey_visibility: String,
    pub rtt_threshold_ms: u64,
    pub p95_rtt_threshold_ms: u64,
    pub uptime_threshold_percent: f64,
    pub critical_uptime_threshold_percent: f64,
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub misskey_url: String,
    pub misskey_token: Option<String>,
    pub target_urls: Vec<String>,
    pub check_interval_seconds: u64,
    pub user_agent: String,
    pub request_timeout_seconds: u64,
    pub output_format: String,
    pub output_path: String,
    pub max_concurrent_checks: usize,
    pub colo_change_notify_misskey: bool,
    pub misskey_concurrent_notifications: usize,
    pub reporting: ReportingSettings,
}

pub fn load_settings() -> Result<Settings> {
    let settings = Config::builder()
        .add_source(File::with_name("config/base"))
        .add_source(File::with_name("config/local"))
        .build()?;
    Ok(settings.try_deserialize()?)
}
