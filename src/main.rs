mod check;
mod cli;
mod config;
mod io;
mod misskey;
mod models;
mod report;

use crate::check::run_checks_once;
use crate::cli::Cli;
use crate::config::load_settings;
use crate::report::run_report_once;
use anyhow::Result;
use clap::Parser;
use humantime::parse_duration;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::Semaphore;
use tokio::time::{self, MissedTickBehavior};
use url::Url;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = load_settings()?;

    if settings.reporting.p95_rtt_threshold_ms < settings.reporting.rtt_threshold_ms {
        anyhow::bail!("p95_rtt_threshold_ms must be greater than or equal to rtt_threshold_ms");
    }

    // URL バリデーション
    for url in &settings.target_urls {
        let parsed = Url::parse(url).map_err(|e| anyhow::anyhow!("Invalid URL {}: {}", url, e))?;
        match parsed.scheme() {
            "http" | "https" => {}
            other => anyhow::bail!("Unsupported URL scheme '{}' for {}", other, url),
        }
    }

    if settings.reporting.enabled && settings.output_format == "none" {
        anyhow::bail!(
            "レポート機能が有効になっていますが、output_format が 'none' に設定されています。\nレポートを使用するには、output_format を 'json' または 'jsonl' に設定してください。"
        );
    }
    let client = Client::builder()
        .user_agent(&settings.user_agent)
        .timeout(Duration::from_secs(settings.request_timeout_seconds))
        .build()?;

    if cli.report {
        run_report_once(&settings, &cli, &client).await?;
        return Ok(());
    }

    println!(
        "Starting tracekey monitoring with User-Agent: {}",
        settings.user_agent
    );

    let check_interval_duration = Duration::from_secs(settings.check_interval_seconds);
    if check_interval_duration.is_zero() {
        anyhow::bail!("Check interval cannot be 0");
    }
    if settings.max_concurrent_checks == 0 {
        anyhow::bail!("max_concurrent_checks cannot be 0");
    }
    if settings.misskey_concurrent_notifications == 0 {
        anyhow::bail!("misskey_concurrent_notifications cannot be 0");
    }
    let misskey_semaphore = Arc::new(Semaphore::new(settings.misskey_concurrent_notifications));
    let mut check_interval = time::interval(check_interval_duration);
    check_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let report_interval_duration = parse_duration(&settings.reporting.interval)?;
    if report_interval_duration.is_zero() {
        anyhow::bail!("Reporting interval cannot be 0");
    }
    let mut report_interval = time::interval(report_interval_duration);

    // Skip the first report tick to delay initial report
    let _ = report_interval.tick().await;

    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                if let Err(e) = run_checks_once(&settings, &client, misskey_semaphore.clone()).await {
                    eprintln!("Scheduled check failed: {}", e);
                }
            },
            _ = report_interval.tick() => {
                if settings.reporting.enabled {
                    println!("Generating periodic report...");
                    if let Err(e) = run_report_once(&settings, &cli, &client).await {
                        eprintln!("Failed to generate periodic report: {}", e);
                    }
                }
            },
            _ = signal::ctrl_c() => {
                println!("\nCtrl+C received, shutting down.");
                break;
            }
        }
    }

    println!("Tracekey monitoring stopped.");
    Ok(())
}
