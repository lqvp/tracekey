use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::Parser;
use colored::*;
use config::{Config, File};
use humantime::parse_duration;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use statistical::{mean, median};
use std::collections::HashMap;
use std::fs::{File as StdFile, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
use tokio::time;

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
    uptime_threshold_percent: f64,
}

#[derive(Debug, Deserialize)]
struct Settings {
    misskey_url: String,
    misskey_token: Option<String>,
    misskey_visibility: String,
    target_urls: Vec<String>,
    check_interval_seconds: u64,
    user_agent: String,
    request_timeout_seconds: u64,
    output_format: String,
    output_path: String,
    reporting: ReportingSettings,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct CheckResult {
    timestamp: DateTime<Utc>,
    url: String,
    colo: String,
    rtt_millis: u128,
    #[serde(flatten)]
    trace_data: HashMap<String, String>,
}

#[derive(Debug)]
struct RttStats {
    min: u128,
    max: u128,
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
    colo_changes: usize,
    most_frequent_colo: String,
}

#[derive(Debug)]
struct Report {
    total_targets: usize,
    overall_uptime: f64,
    target_stats: Vec<TargetStats>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = load_settings()?;

    if settings.reporting.enabled && settings.output_format == "none" {
        anyhow::bail!(
            "Reporting is enabled, but output_format is set to 'none'. Please use 'json' or 'csv'."
        );
    }

    if cli.report {
        run_report_once(&settings, &cli).await?;
        return Ok(());
    }

    let mut last_colos: HashMap<String, String> = HashMap::new();
    let client = Client::builder()
        .user_agent(&settings.user_agent)
        .timeout(Duration::from_secs(settings.request_timeout_seconds))
        .build()?;

    println!("Starting tracekey monitoring with User-Agent: {}", settings.user_agent);

    let check_interval_duration = Duration::from_secs(settings.check_interval_seconds);
    let mut check_interval = time::interval(check_interval_duration);

    let report_interval_duration = parse_duration(&settings.reporting.interval)?;
    let mut report_interval = time::interval(report_interval_duration);

    loop {
        tokio::select! {
            _ = check_interval.tick() => {
                println!("Running check...");
                let mut results: Vec<CheckResult> = Vec::new();
                let mut changes: Vec<String> = Vec::new();

                for url in &settings.target_urls {
                    println!("Checking trace for {}", url);
                    match get_cloudflare_trace(&client, url).await {
                        Ok(result) => {
                            println!(
                                "Result for {}: colo={}, rtt={}ms",
                                result.url, result.colo, result.rtt_millis
                            );
                            let last_colo = last_colos.entry(result.url.clone()).or_insert_with(|| result.colo.clone());
                            if !last_colo.is_empty() && last_colo != &result.colo {
                                let message = format!(
                                    "Cloudflare colocation for {} changed: `{}` -> `{}` (RTT: {}ms)",
                                    result.url, last_colo, result.colo, result.rtt_millis
                                );
                                println!("CHANGE DETECTED: {}", message);
                                changes.push(message);
                            }
                            *last_colo = result.colo.clone();
                            results.push(result);
                        }
                        Err(e) => eprintln!("Failed to get trace for {}: {}", url, e),
                    }
                }

                if !results.is_empty() {
                    if let Err(e) = write_results(&settings.output_path, &settings.output_format, &results) {
                        eprintln!("Failed to write results: {}", e);
                    }
                }

                if !changes.is_empty() {
                    if let Some(token) = &settings.misskey_token {
                        if !token.is_empty() {
                            let final_message = changes.join("\n");
                            println!("Posting to Misskey:\n{}", final_message);
                            if let Err(e) =
                                post_to_misskey(&client, &settings.misskey_url, token, &final_message, &settings.misskey_visibility)
                                    .await
                            {
                                eprintln!("Failed to post to Misskey: {}", e);
                            }
                        }
                    }
                }
            },
            _ = report_interval.tick() => {
                if settings.reporting.enabled {
                    println!("Generating periodic report...");
                    if let Err(e) = run_report_once(&settings, &cli).await {
                        eprintln!("Failed to generate periodic report: {}", e);
                    }
                }
            }
        }
    }
}

async fn run_report_once(settings: &Settings, cli: &Cli) -> Result<()> {
    let until = cli.until.unwrap_or_else(Utc::now);
    let since = cli.since.unwrap_or_else(|| {
        until
            - ChronoDuration::from_std(parse_duration(&settings.reporting.interval).unwrap())
                .unwrap()
    });

    let filtered_results =
        match load_check_results(&settings.output_path, &settings.output_format, Some(since), Some(until)) {
            Ok(r) => r,
            Err(e) => {
                if e.downcast_ref::<std::io::Error>()
                    .map_or(true, |io_err| io_err.kind() != std::io::ErrorKind::NotFound)
                {
                    eprintln!("Could not load check results: {}. No report will be generated.", e);
                }
                return Ok(());
            }
        };

    if filtered_results.is_empty() {
        println!("No data found for the specified period. No report will be generated.");
        return Ok(());
    }

    let report = generate_report(&filtered_results, &settings.target_urls);

    if settings.reporting.output_to_console {
        format_report_console(&report, &settings.reporting);
    }

    if settings.reporting.output_to_misskey {
        let mfm_report = format_report_mfm(&report);
        if cli.dry_run {
            println!("\n--- Misskey Dry Run ---\n{}", mfm_report);
        } else if let Some(token) = &settings.misskey_token {
            if !token.is_empty() {
                let client = Client::new();
                println!("Posting report to Misskey...");
                post_to_misskey(
                    &client,
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

fn generate_report(results: &[CheckResult], targets: &[String]) -> Report {
    let mut target_stats = Vec::new();

    for target in targets {
        let target_results: Vec<_> = results.iter().filter(|r| &r.url == target).cloned().collect();
        if target_results.is_empty() {
            continue;
        }

        let total_checks = target_results.len();
        let successful_checks = total_checks; // Assuming all logged checks are successful for now
        let uptime = (successful_checks as f64 / total_checks as f64) * 100.0;

        let rtts: Vec<f64> = target_results.iter().map(|r| r.rtt_millis as f64).collect();
        let mut sorted_rtts = rtts.clone();
        sorted_rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95_index = (sorted_rtts.len() as f64 * 0.95) as usize;
        let p95 = if p95_index < sorted_rtts.len() {
            sorted_rtts[p95_index]
        } else {
            *sorted_rtts.last().unwrap_or(&0.0)
        };
        let rtt_stats = RttStats {
            min: target_results.iter().map(|r| r.rtt_millis).min().unwrap_or(0),
            max: target_results.iter().map(|r| r.rtt_millis).max().unwrap_or(0),
            mean: mean(&rtts),
            median: median(&rtts),
            p95,
        };

        let mut unique_colos = HashMap::new();
        let mut colo_changes = 0;
        let mut last_colo = "";
        for r in &target_results {
            *unique_colos.entry(r.colo.clone()).or_insert(0) += 1;
            if !last_colo.is_empty() && last_colo != r.colo {
                colo_changes += 1;
            }
            last_colo = &r.colo;
        }
        let most_frequent_colo = unique_colos.iter().max_by_key(|(_, count)| *count).map(|(colo, _)| colo.clone()).unwrap_or_default();

        target_stats.push(TargetStats {
            url: target.clone(),
            total_checks,
            successful_checks,
            uptime,
            rtt_stats,
            unique_colos: unique_colos.keys().cloned().collect(),
            colo_changes,
            most_frequent_colo,
        });
    }

    let overall_uptime = if !target_stats.is_empty() {
        target_stats.iter().map(|s| s.uptime).sum::<f64>() / target_stats.len() as f64
    } else {
        0.0
    };

    Report {
        total_targets: targets.len(),
        overall_uptime,
        target_stats,
    }
}

fn format_report_mfm(report: &Report) -> String {
    let mut mfm = String::new();
    mfm.push_str(&format!(
        "**📊 監視レポート**\n\n**総合サマリー**\n- **監視対象:** {} サイト\n- **全体の平均稼働率:** {:.3}%\n\n",
        report.total_targets, report.overall_uptime
    ));

    for stats in &report.target_stats {
        mfm.push_str(&format!("**?[{}]({})**\n", stats.url, stats.url));
        mfm.push_str(&format!(
            "- **稼働率:** {:.3}% ({} / {} 成功)\n",
            stats.uptime, stats.successful_checks, stats.total_checks
        ));
        mfm.push_str(&format!(
            "- **RTT:** Min: {}ms, Max: {}ms, Avg: {:.2}ms, Median: {:.2}ms, P95: {:.2}ms\n",
            stats.rtt_stats.min, stats.rtt_stats.max, stats.rtt_stats.mean, stats.rtt_stats.median, stats.rtt_stats.p95
        ));
        mfm.push_str(&format!(
            "- **Colo:** {}回変更, 最頻出: {}, ユニーク: {}\n\n",
            stats.colo_changes,
            stats.most_frequent_colo,
            stats.unique_colos.join(", ")
        ));
    }

    mfm
}

fn format_report_console(report: &Report, settings: &ReportingSettings) {
    println!("📊 監視レポート");
    println!("-----------------");
    println!(
        "総合サマリー: {} サイト, 平均稼働率: {:.3}%",
        report.total_targets, report.overall_uptime
    );
    println!("-----------------");

    for stats in &report.target_stats {
        let uptime_str = format!("{:.3}%", stats.uptime);
        let uptime_colored = if stats.uptime < settings.uptime_threshold_percent {
            uptime_str.yellow()
        } else {
            uptime_str.green()
        };

        let rtt_avg_str = format!("{:.2}ms", stats.rtt_stats.mean);
        let rtt_avg_colored = if stats.rtt_stats.mean > settings.rtt_threshold_ms as f64 {
            rtt_avg_str.red()
        } else {
            rtt_avg_str.green()
        };

        println!("URL: {}", stats.url.bold());
        println!("  稼働率: {}", uptime_colored);
        println!("  RTT - Min: {}ms, Max: {}ms, Avg: {}, Median: {:.2}ms, P95: {:.2}ms",
                 stats.rtt_stats.min, stats.rtt_stats.max, rtt_avg_colored, stats.rtt_stats.median, stats.rtt_stats.p95);
        println!("  Colo Changes: {}", stats.colo_changes);
        println!("  Most Frequent Colo: {}", stats.most_frequent_colo);
    }
}

fn load_settings() -> Result<Settings> {
    let settings = Config::builder()
        .add_source(File::with_name("config/default"))
        .build()?;
    Ok(settings.try_deserialize()?)
}

async fn get_cloudflare_trace(client: &Client, url: &str) -> Result<CheckResult> {
    let trace_url = format!("{}/cdn-cgi/trace", url);
    let start_time = time::Instant::now();
    let resp = client.get(&trace_url).send().await?.text().await?;
    let rtt = start_time.elapsed();

    let mut trace_data = HashMap::new();
    for line in resp.lines() {
        if let Some((key, value)) = line.split_once('=') {
            trace_data.insert(key.to_string(), value.to_string());
        }
    }

    let colo = trace_data.remove("colo").unwrap_or_else(|| "N/A".to_string());

    Ok(CheckResult {
        timestamp: Utc::now(),
        url: url.to_string(),
        colo,
        rtt_millis: rtt.as_millis(),
        trace_data,
    })
}

async fn post_to_misskey(client: &Client, url: &str, token: &str, text: &str, visibility: &str) -> Result<()> {
    let api_url = format!("{}/api/notes/create", url);
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
            return Err(anyhow::anyhow!("Failed to post to Misskey after {} attempts", max_attempts));
        }

        time::sleep(delay).await;
        delay *= 2;
    }
}

fn write_results(path: &str, format: &str, results: &[CheckResult]) -> Result<()> {
    if format == "none" {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    match format {
        "json" => {
            for result in results {
                let json_string = serde_json::to_string(result)?;
                writeln!(file, "{}", json_string)?;
            }
        }
        "csv" => {
            // Check if the file is new to write headers
            let is_new_file = file.metadata()?.len() == 0;
            let mut wtr = csv::WriterBuilder::new()
                .has_headers(is_new_file)
                .from_writer(file);
            for result in results {
                wtr.serialize(result)?;
            }
            wtr.flush()?;
        }
        _ => {}
    }
    Ok(())
}

fn load_check_results(
    path: &str,
    format: &str,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<Vec<CheckResult>> {
    let file = StdFile::open(path)?;
    let reader = BufReader::new(file);
    let mut results = Vec::new();

    match format {
        "json" => {
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let result: CheckResult = serde_json::from_str(&line)?;
                let in_since = since.map_or(true, |s| result.timestamp >= s);
                let in_until = until.map_or(true, |u| result.timestamp <= u);

                if in_since && in_until {
                    results.push(result);
                }
            }
        }
        "csv" => {
            let mut rdr = csv::Reader::from_reader(reader);
            for result in rdr.deserialize::<CheckResult>() {
                let result = result?;
                let in_since = since.map_or(true, |s| result.timestamp >= s);
                let in_until = until.map_or(true, |u| result.timestamp <= u);

                if in_since && in_until {
                    results.push(result);
                }
            }
        }
        _ => {}
    }
    Ok(results)
}
