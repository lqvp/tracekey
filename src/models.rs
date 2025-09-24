use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct CheckResult {
    pub(crate) timestamp: DateTime<Utc>,
    pub(crate) url: String,
    pub(crate) success: bool,
    pub(crate) rtt_millis: Option<u64>,
    pub(crate) error: Option<String>,
    pub(crate) colo: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LastSuccessState {
    pub(crate) url: String,
    pub(crate) colo: Option<String>,
    pub(crate) timestamp: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub(crate) last_notification_timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct RttStats {
    pub(crate) min: u64,
    pub(crate) max: u64,
    pub(crate) mean: f64,
    pub(crate) median: f64,
    pub(crate) p95: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct TargetStats {
    pub(crate) url: String,
    pub(crate) total_checks: usize,
    pub(crate) successful_checks: usize,
    pub(crate) uptime: f64,
    pub(crate) rtt_stats: RttStats,
    pub(crate) unique_colos: Vec<String>,
    pub(crate) colo_transitions: usize,
    pub(crate) most_frequent_colo: String,
}

#[derive(Debug, Clone)]
pub(crate) struct Report {
    pub(crate) since: DateTime<Utc>,
    pub(crate) until: DateTime<Utc>,
    pub(crate) configured_targets: usize,
    pub(crate) reported_targets: usize,
    pub(crate) overall_uptime: f64,
    pub(crate) target_stats: Vec<TargetStats>,
}
