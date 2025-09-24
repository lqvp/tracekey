use anyhow::Result;
use config::{Config, File};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ReportingSettings {
    pub(crate) enabled: bool,
    pub(crate) interval: String,
    pub(crate) output_to_console: bool,
    pub(crate) output_to_misskey: bool,
    pub(crate) misskey_visibility: String,
    pub(crate) rtt_threshold_ms: u64,
    pub(crate) p95_rtt_threshold_ms: u64,
    pub(crate) uptime_threshold_percent: f64,
    pub(crate) critical_uptime_threshold_percent: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Settings {
    pub(crate) misskey_url: String,
    pub(crate) misskey_token: Option<String>,
    pub(crate) target_urls: Vec<String>,
    pub(crate) check_interval_seconds: u64,
    pub(crate) user_agent: String,
    pub(crate) request_timeout_seconds: u64,
    pub(crate) output_format: String,
    pub(crate) output_path: String,
    pub(crate) max_concurrent_checks: usize,
    pub(crate) colo_change_notify_misskey: bool, // separate immediate notification toggle
    pub(crate) misskey_concurrent_notifications: usize,
    pub(crate) reporting: ReportingSettings,
}

pub(crate) fn load_settings() -> Result<Settings> {
    let settings = Config::builder()
        .add_source(File::with_name("config/default.toml").required(false))
        .add_source(config::Environment::with_prefix("APP").separator("__"))
        .build()?;
    Ok(settings.try_deserialize()?)
}
