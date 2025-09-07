use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckResult {
    pub timestamp: DateTime<Utc>,
    pub url: String,
    pub success: bool,
    pub rtt_millis: Option<u64>,
    pub error: Option<String>,
    pub colo: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LastSuccessState {
    pub url: String,
    pub colo: Option<String>,
    pub timestamp: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub last_notification_timestamp: DateTime<Utc>,
}

#[derive(Debug)]
pub struct RttStats {
    pub min: u64,
    pub max: u64,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
}

#[derive(Debug)]
pub struct TargetStats {
    pub url: String,
    pub total_checks: usize,
    pub successful_checks: usize,
    pub uptime: f64,
    pub rtt_stats: RttStats,
    pub unique_colos: Vec<String>,
    pub colo_transitions: usize,
    pub most_frequent_colo: String,
}

#[derive(Debug)]
pub struct Report {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub configured_targets: usize,
    pub reported_targets: usize,
    pub overall_uptime: f64,
    pub target_stats: Vec<TargetStats>,
}
