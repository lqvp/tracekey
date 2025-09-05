use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use clap::Parser;
use colored::*;
use config::{Config, File};
use futures::stream::StreamExt;
use humantime::parse_duration;
use rand::{Rng, rng};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use statistical::{mean, median};
use std::collections::HashMap;
use std::fs::{File as StdFile, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
use tokio::time::{self, MissedTickBehavior};
use url::Url;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(long)]
    report: bool,
    #[arg(long)]
    since: Option<DateTime<Utc>>,
    #[arg(long)]
    until: Option<DateTime<Utc>>,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
struct ReportingSettings {
    enabled: bool,
    interval: String,
    output_to_console: bool,
    output_to_misskey: bool,
    misskey_visibility: String,
    rtt_threshold_ms: u64,
    p95_rtt_threshold_ms: u64,
    uptime_threshold_percent: f64,
    critical_uptime_threshold_percent: f64,
}

#[derive(Debug, Deserialize)]
struct Settings {
    misskey_url: String,
    misskey_token: Option<String>,
    target_urls: Vec<String>,
    check_interval_seconds: u64,
    user_agent: String,
    request_timeout_seconds: u64,
    output_format: String,
    output_path: String,
    max_concurrent_checks: usize,
    colo_change_notify_misskey: bool, // 即時通知の設定を分離
    reporting: ReportingSettings,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
struct CheckResult {
    timestamp: DateTime<Utc>,
    url: String,
    success: bool,
    rtt_millis: Option<u64>,
    error: Option<String>,
    colo: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LastSuccessState {
    url: String,
    colo: Option<String>,
    timestamp: DateTime<Utc>,
}

#[derive(Debug)]
struct RttStats {
    min: u64,
    max: u64,
    mean: f64,
    median: f64,
    p95: f64,
}

#[derive(Debug)]
struct TargetStats {
    url: String,
    total_checks: usize,
    successful_checks: usize,
    uptime: f64,
    rtt_stats: RttStats,
    unique_colos: Vec<String>,
    colo_transitions: usize,
    most_frequent_colo: String,
}

#[derive(Debug)]
struct Report {
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    configured_targets: usize,
    reported_targets: usize,
    overall_uptime: f64,
    target_stats: Vec<TargetStats>,
}

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
    let mut check_interval = time::interval(check_interval_duration);
    check_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let report_interval_duration = parse_duration(&settings.reporting.interval)?;
    if report_interval_duration.is_zero() {
        anyhow::bail!("Reporting interval cannot be 0");
    }
    let mut report_interval = time::interval(report_interval_duration);

    // Run initial check immediately
    if let Err(e) = run_checks_once(&settings, &client, false).await {
        eprintln!("Initial check failed: {}", e);
    }

    // Skip the first report tick to delay initial report
    let _ = report_interval.tick().await;

    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                if let Err(e) = run_checks_once(&settings, &client, false).await {
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
            _ = tokio::signal::ctrl_c() => {
                println!("\nCtrl+C received, shutting down.");
                break;
            }
        }
    }

    println!("Tracekey monitoring stopped.");
    Ok(())
}

async fn run_checks_once(settings: &Settings, client: &Client, dry_run: bool) -> Result<()> {
    println!("Running check...");

    let prev_map: HashMap<String, Option<String>> = load_last_success_states()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|state| (state.url, state.colo))
        .collect();

    let tasks = settings.target_urls.iter().cloned().map(|url| {
        let client = client.clone();
        async move {
            let res = get_cloudflare_trace(&client, &url).await;
            (url, res)
        }
    });
    let outcomes = futures::stream::iter(tasks)
        .buffer_unordered(settings.max_concurrent_checks)
        .collect::<Vec<_>>()
        .await;

    let mut results: Vec<CheckResult> = Vec::new();
    for outcome in outcomes {
        match outcome {
            (_url, Ok(result)) => {
                println!(
                    "Result for {}: colo={}, rtt={}ms",
                    result.url,
                    result.colo.as_deref().unwrap_or("N/A"),
                    result.rtt_millis.unwrap_or(0)
                );
                results.push(result);
            }
            (url, Err(e)) => {
                eprintln!("Failed to get trace for {}: {}", url, e);
                results.push(CheckResult {
                    timestamp: Utc::now(),
                    url,
                    success: false,
                    rtt_millis: None,
                    error: Some(e.to_string()),
                    colo: None,
                });
            }
        }
    }
    if !dry_run {
        // Colo変更検知とMisskey投稿
        for result in &results {
            if result.success {
                if let (Some(curr), Some(prev)) = (
                    result.colo.as_ref(),
                    prev_map.get(&result.url).and_then(|o| o.as_ref()),
                ) {
                    if curr != prev {
                        let message = format!(
                            "Cloudflare colocation for ?[{}]({}) changed: `{}` -> `{}` (RTT: {}ms)",
                            result.url,
                            result.url,
                            prev,
                            curr,
                            result.rtt_millis.unwrap_or(0)
                        );
                        if settings.colo_change_notify_misskey {
                            if let Some(token) = &settings.misskey_token {
                                if !token.is_empty() {
                                    // バックグラウンドでMisskey通知を送信
                                    let misskey_client = client.clone();
                                    let misskey_url = settings.misskey_url.clone();
                                    let misskey_token = token.clone();
                                    let misskey_visibility =
                                        settings.reporting.misskey_visibility.clone();
                                    let message_clone = message.clone();

                                    tokio::spawn(async move {
                                        println!("Posting colo change to Misskey...");
                                        match post_to_misskey(
                                            &misskey_client,
                                            &misskey_url,
                                            &misskey_token,
                                            &message_clone,
                                            &misskey_visibility,
                                        )
                                        .await
                                        {
                                            Ok(_) => println!(
                                                "Colo change posted to Misskey successfully."
                                            ),
                                            Err(e) => eprintln!(
                                                "Failed to post colo change to Misskey: {}",
                                                e
                                            ),
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        // 最後の成功状態を更新
        let success_states: Vec<LastSuccessState> = results
            .iter()
            .filter(|r| r.success)
            .map(|r| LastSuccessState {
                url: r.url.clone(),
                colo: r.colo.clone(),
                timestamp: r.timestamp,
            })
            .collect();

        if !success_states.is_empty() {
            if let Err(e) = save_last_success_states(&success_states).await {
                eprintln!("Failed to save last success states: {}", e);
            }
        }
    }

    if !results.is_empty() {
        if let Err(e) = write_results(
            settings.output_path.clone(),
            settings.output_format.clone(),
            results,
        )
        .await
        {
            eprintln!("Failed to write results: {}", e);
        }
    }

    Ok(())
}

async fn run_report_once(settings: &Settings, cli: &Cli, client: &Client) -> Result<()> {
    let until = cli.until.unwrap_or_else(Utc::now);
    let since = if let Some(s) = cli.since {
        s
    } else {
        let duration_std = parse_duration(&settings.reporting.interval)
            .map_err(|e| anyhow::anyhow!("Failed to parse reporting interval setting: {}", e))?;

        let duration_chrono = ChronoDuration::from_std(duration_std)
            .map_err(|_| anyhow::anyhow!("Reporting interval setting is invalid or too large"))?;

        until - duration_chrono
    };

    if since > until {
        anyhow::bail!(
            "--since ({}) must be earlier than or equal to --until ({})",
            since,
            until
        );
    }

    let filtered_results = match load_check_results(
        settings.output_path.clone(),
        settings.output_format.clone(),
        Some(since),
        Some(until),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            if e.downcast_ref::<std::io::Error>()
                .map_or(true, |io_err| io_err.kind() != std::io::ErrorKind::NotFound)
            {
                eprintln!(
                    "Could not load check results: {}. No report will be generated.",
                    e
                );
            }
            return Ok(());
        }
    };

    if filtered_results.is_empty() {
        println!("No data found for the specified period. No report will be generated.");
        return Ok(());
    }

    let report = generate_report(&filtered_results, &settings.target_urls, since, until);

    if settings.reporting.output_to_console {
        format_report_console(&report, &settings.reporting);
    }

    if settings.reporting.output_to_misskey {
        let mfm_report = format_report_mfm(&report);
        if cli.dry_run {
            println!("\n--- Misskey Dry Run ---\n{}", mfm_report);
        } else if let Some(token) = &settings.misskey_token {
            if !token.is_empty() {
                println!("Posting report to Misskey...");
                post_to_misskey(
                    client,
                    &settings.misskey_url,
                    token,
                    &mfm_report,
                    &settings.reporting.misskey_visibility,
                )
                .await?;
                println!("Report posted to Misskey successfully.");
            }
        }
    }

    Ok(())
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let p = p.clamp(0.0, 1.0);
    let n = sorted.len();
    let rank = p * (n as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let w = rank - lo as f64;
        sorted[lo] * (1.0 - w) + sorted[hi] * w
    }
}

fn generate_report(
    results: &[CheckResult],
    targets: &[String],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Report {
    let mut target_stats = Vec::new();
    for target in targets {
        let mut target_results: Vec<_> = results
            .iter()
            .filter(|r| &r.url == target)
            .cloned()
            .collect();
        if target_results.is_empty() {
            continue;
        }

        // 時系列でソートして正確なcolo遷移を計算
        target_results.sort_by_key(|r| r.timestamp);

        let total_checks = target_results.len();
        let successful_checks = target_results.iter().filter(|r| r.success).count();
        let uptime = if total_checks > 0 {
            (successful_checks as f64 / total_checks as f64) * 100.0
        } else {
            0.0
        };

        let rtts: Vec<f64> = target_results
            .iter()
            .filter_map(|r| r.rtt_millis.map(|rtt| rtt as f64))
            .collect();

        let rtt_stats = if !rtts.is_empty() {
            let mut sorted_rtts = rtts.clone();
            sorted_rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p95 = percentile(&sorted_rtts, 0.95);

            RttStats {
                min: target_results
                    .iter()
                    .filter_map(|r| r.rtt_millis)
                    .min()
                    .unwrap_or(0),
                max: target_results
                    .iter()
                    .filter_map(|r| r.rtt_millis)
                    .max()
                    .unwrap_or(0),
                mean: mean(&rtts),
                median: median(&rtts),
                p95,
            }
        } else {
            RttStats {
                min: 0,
                max: 0,
                mean: 0.0,
                median: 0.0,
                p95: 0.0,
            }
        };
        // 実際の観測回数ベースで最頻出coloを算出
        let mut colo_frequency = std::collections::HashMap::new();
        for r in &target_results {
            if let Some(ref colo) = r.colo {
                *colo_frequency.entry(colo.clone()).or_insert(0) += 1;
            }
        }

        let most_frequent_colo = colo_frequency
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(colo, _)| colo.clone())
            .unwrap_or_default();

        let mut unique_colos_list: Vec<_> = colo_frequency.keys().cloned().collect();
        unique_colos_list.sort();

        // colo遷移回数を算出
        let mut colo_transitions = 0;
        let mut last_colo = None;
        for r in &target_results {
            if let Some(ref colo) = r.colo {
                if let Some(ref last) = last_colo {
                    if last != colo {
                        colo_transitions += 1;
                    }
                }
                last_colo = Some(colo.clone());
            }
        }
        target_stats.push(TargetStats {
            url: target.clone(),
            total_checks,
            successful_checks,
            uptime,
            rtt_stats,
            unique_colos: unique_colos_list,
            colo_transitions,
            most_frequent_colo,
        });
    }

    let (succ, total): (usize, usize) = target_stats
        .iter()
        .map(|s| (s.successful_checks, s.total_checks))
        .fold((0, 0), |(a, b), (x, y)| (a + x, b + y));

    let overall_uptime = if total > 0 {
        (succ as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    Report {
        since,
        until,
        configured_targets: targets.len(),
        reported_targets: target_stats.len(),
        overall_uptime,
        target_stats,
    }
}

fn format_report_mfm(report: &Report) -> String {
    let mut mfm = String::new();

    // 期間情報をローカル時刻で表示
    let since_local = report.since.with_timezone(&Local);
    let until_local = report.until.with_timezone(&Local);

    mfm.push_str(&format!(
        "**📊 監視レポート**\n**期間:** {} ～ {}\n\n**総合サマリー**\n- **監視対象:** {} / {} サイト\n- **全体の平均稼働率:** {:.3}%\n\n",
        since_local.format("%Y-%m-%d %H:%M:%S %Z"),
        until_local.format("%Y-%m-%d %H:%M:%S %Z"),
        report.reported_targets, report.configured_targets, report.overall_uptime
    ));

    for stats in &report.target_stats {
        mfm.push_str(&format!("**?[{}]({})**\n", stats.url, stats.url));
        mfm.push_str(&format!(
            "- **稼働率:** {:.3}% ({} / {} 成功)\n",
            stats.uptime, stats.successful_checks, stats.total_checks
        ));
        mfm.push_str(&format!(
            "- **RTT:** Min: {}ms, Max: {}ms, Avg: {:.2}ms, Median: {:.2}ms, P95: {:.2}ms\n",
            stats.rtt_stats.min,
            stats.rtt_stats.max,
            stats.rtt_stats.mean,
            stats.rtt_stats.median,
            stats.rtt_stats.p95
        ));
        mfm.push_str(&format!(
            "- **Colo:** {}回遷移, 最頻出: {}, ユニーク: {}\n\n",
            stats.colo_transitions,
            stats.most_frequent_colo,
            stats.unique_colos.join(", ")
        ));
    }

    mfm
}

fn format_report_console(report: &Report, settings: &ReportingSettings) {
    // 期間情報をローカル時刻で表示
    let since_local = report.since.with_timezone(&Local);
    let until_local = report.until.with_timezone(&Local);

    println!("📊 監視レポート");
    println!("-----------------");
    println!(
        "期間: {} ～ {}",
        since_local.format("%Y-%m-%d %H:%M:%S %Z"),
        until_local.format("%Y-%m-%d %H:%M:%S %Z")
    );
    println!(
        "総合サマリー: {} / {} サイト, 平均稼働率: {:.3}%",
        report.reported_targets, report.configured_targets, report.overall_uptime
    );
    println!("-----------------");

    for stats in &report.target_stats {
        let uptime_str = format!("{:.3}%", stats.uptime);
        let uptime_colored = if stats.uptime < settings.critical_uptime_threshold_percent {
            uptime_str.red()
        } else if stats.uptime < settings.uptime_threshold_percent {
            uptime_str.yellow()
        } else {
            uptime_str.green()
        };

        let rtt_avg_str = format!("{:.2}ms", stats.rtt_stats.mean);
        let rtt_p95_str = format!("{:.2}ms", stats.rtt_stats.p95);
        let rtt_avg_colored = if stats.rtt_stats.mean > settings.rtt_threshold_ms as f64 {
            rtt_avg_str.red()
        } else {
            rtt_avg_str.green()
        };
        let rtt_p95_colored = if stats.rtt_stats.p95 > settings.p95_rtt_threshold_ms as f64 {
            rtt_p95_str.red()
        } else {
            rtt_p95_str.green()
        };

        println!("URL: {}", stats.url.bold());
        println!("  稼働率: {}", uptime_colored);
        println!(
            "  RTT - Min: {}ms, Max: {}ms, Avg: {}, Median: {:.2}ms, P95: {}",
            stats.rtt_stats.min,
            stats.rtt_stats.max,
            rtt_avg_colored,
            stats.rtt_stats.median,
            rtt_p95_colored
        );
        let most = if stats.most_frequent_colo.is_empty() {
            "N/A"
        } else {
            &stats.most_frequent_colo
        };
        let uniques = if stats.unique_colos.is_empty() {
            "N/A".to_string()
        } else {
            stats.unique_colos.join(", ")
        };
        println!("  Colo Transitions: {}", stats.colo_transitions);
        println!("  Most Frequent Colo: {}", most);
        println!("  Unique Colos: {}", uniques);
    }
}

fn load_settings() -> Result<Settings> {
    let settings = Config::builder()
        .add_source(File::with_name("config/default.toml").required(false))
        .add_source(config::Environment::with_prefix("APP").separator("__"))
        .build()?;
    Ok(settings.try_deserialize()?)
}

async fn get_cloudflare_trace(client: &Client, url: &str) -> Result<CheckResult> {
    let base_url = Url::parse(url)?;
    let trace_url = base_url.join("/cdn-cgi/trace")?.to_string();
    let start_time = time::Instant::now();
    let resp = client
        .get(&trace_url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let rtt = start_time.elapsed();

    let colo_opt = resp
        .lines()
        .find_map(|line| line.strip_prefix("colo="))
        .map(|s| s.to_string());

    Ok(CheckResult {
        timestamp: Utc::now(),
        url: url.to_string(),
        success: true,
        rtt_millis: Some(std::cmp::min(rtt.as_millis(), u64::MAX as u128) as u64),
        error: None,
        colo: colo_opt,
    })
}

async fn post_to_misskey(
    client: &Client,
    url: &str,
    token: &str,
    text: &str,
    visibility: &str,
) -> Result<()> {
    let base_url = Url::parse(url)?;
    let api_url = base_url.join("/api/notes/create")?.to_string();
    let mut params = HashMap::new();
    params.insert("i", token.to_string());
    params.insert("text", text.to_string());
    params.insert("visibility", visibility.to_string());

    let mut attempts = 0;
    let max_attempts = 5;
    let mut delay = Duration::from_secs(1);

    loop {
        attempts += 1;
        let response = client.post(&api_url).json(&params).send().await;

        match response {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            Ok(resp) => {
                let status = resp.status();
                let error_text = resp.text().await.unwrap_or_else(|_| "No body".to_string());
                eprintln!(
                    "Attempt {} failed: Misskey API returned status {} - {}",
                    attempts, status, error_text
                );
            }
            Err(e) => {
                eprintln!("Attempt {} failed: Request error: {}", attempts, e);
            }
        }

        if attempts >= max_attempts {
            return Err(anyhow::anyhow!(
                "Failed to post to Misskey after {} attempts",
                max_attempts
            ));
        }

        time::sleep(delay).await;
        // jitter は u64 を明示し、Duration は飽和演算で安全に拡大
        let jitter_ms: u64 = rng().random_range(0u64..1000u64);
        delay = delay
            .saturating_mul(2)
            .saturating_add(Duration::from_millis(jitter_ms));
    }
}

async fn write_results(path: String, format: String, results: Vec<CheckResult>) -> Result<()> {
    if format == "none" {
        return Ok(());
    }

    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        match format.as_str() {
            "json" | "jsonl" => {
                let mut file = std::io::BufWriter::new(file);
                for result in &results {
                    serde_json::to_writer(&mut file, result)?;
                    file.write_all(b"\n")?;
                }
            }
            other => anyhow::bail!("unsupported output_format: {}", other),
        }
        Ok(())
    })
    .await??;
    Ok(())
}

async fn load_check_results(
    path: String,
    format: String,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<Vec<CheckResult>> {
    let results = tokio::task::spawn_blocking(move || -> Result<Vec<CheckResult>> {
        // ファイルがない場合は空
        let file = match StdFile::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let reader = BufReader::new(file);
        let mut results = Vec::new();

        match format.as_str() {
            "json" | "jsonl" => {
                for (lineno, line) in reader.lines().enumerate() {
                    let line = line?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<CheckResult>(&line) {
                        Ok(result) => {
                            let in_since = since.map_or(true, |s| result.timestamp >= s);
                            let in_until = until.map_or(true, |u| result.timestamp <= u);
                            if in_since && in_until {
                                results.push(result);
                            }
                        }
                        Err(e) => {
                            eprintln!("Skip malformed line {}: {}", lineno + 1, e);
                        }
                    }
                }
            }
            other => anyhow::bail!("unsupported output_format: {}", other),
        }
        Ok(results)
    })
    .await??;

    Ok(results)
}

async fn save_last_success_states(states: &[LastSuccessState]) -> Result<()> {
    let state_dir = "state".to_string();
    let state_file = format!("{}/last_success.json", state_dir);
    let states = states.to_vec();

    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&state_dir)?;

        let mut all_states: HashMap<String, LastSuccessState> = HashMap::new();

        // 既存の状態を読み込み
        if let Ok(file) = StdFile::open(&state_file) {
            let reader = BufReader::new(file);
            if let Ok(existing_states) = serde_json::from_reader::<_, Vec<LastSuccessState>>(reader)
            {
                for state in existing_states {
                    all_states.insert(state.url.clone(), state);
                }
            }
        }

        // 新しい状態で更新
        for state in &states {
            all_states.insert(state.url.clone(), state.clone());
        }

        let updated_states: Vec<LastSuccessState> = all_states.into_values().collect();

        let tmp_file = format!("{}/last_success.json.tmp", state_dir);
        {
            let file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_file)?;
            serde_json::to_writer_pretty(file, &updated_states)?;
        }
        // アトミック入替
        std::fs::rename(&tmp_file, &state_file)?;
        Ok(())
    })
    .await??;

    Ok(())
}

async fn load_last_success_states() -> Result<Vec<LastSuccessState>> {
    let state_file = "state/last_success.json";

    tokio::task::spawn_blocking(move || -> Result<Vec<LastSuccessState>> {
        let file = match StdFile::open(state_file) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let reader = BufReader::new(file);
        let states = serde_json::from_reader(reader).unwrap_or_default();

        Ok(states)
    })
    .await?
}
