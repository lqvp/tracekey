use crate::cli::Cli;
use crate::config::{ReportingSettings, Settings};
use crate::io::load_check_results;
use crate::misskey::post_to_misskey;
use crate::models::{CheckResult, Report, RttStats, TargetStats};
use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use colored::*;
use humantime::parse_duration;
use reqwest::Client;
use statistical::{mean, median};
use std::collections::HashMap;

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = p / 100.0 * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        sorted[lower] + (sorted[upper] - sorted[lower]) * (rank - lower as f64)
    }
}

fn generate_report(
    results: &[CheckResult],
    target_urls: &[String],
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Report {
    let mut target_map: HashMap<String, Vec<&CheckResult>> = HashMap::new();
    for result in results {
        target_map
            .entry(result.url.clone())
            .or_default()
            .push(result);
    }

    let mut target_stats = Vec::new();
    let mut total_successful_checks = 0;
    let mut total_checks = 0;

    for url in target_urls {
        if let Some(entries) = target_map.get(url) {
            let mut successful_checks = 0;
            let mut rtts = Vec::new();
            let mut unique_colos = Vec::new();
            let mut colo_transitions = 0;
            let mut last_colo: Option<&String> = None;

            for result in entries {
                total_checks += 1;
                if result.success {
                    successful_checks += 1;
                    total_successful_checks += 1;
                    if let Some(rtt) = result.rtt_millis {
                        rtts.push(rtt as f64);
                    }
                    if let Some(colo) = &result.colo {
                        if !unique_colos.contains(colo) {
                            unique_colos.push(colo.clone());
                        }
                        if let Some(last) = last_colo {
                            if last != colo {
                                colo_transitions += 1;
                            }
                        }
                        last_colo = Some(colo);
                    }
                }
            }

            let rtt_stats = if !rtts.is_empty() {
                rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
                RttStats {
                    min: rtts.first().copied().unwrap_or(0.0) as u64,
                    max: rtts.last().copied().unwrap_or(0.0) as u64,
                    mean: mean(&rtts),
                    median: median(&rtts),
                    p95: percentile(&rtts, 95.0),
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

            let most_frequent_colo = entries
                .iter()
                .filter_map(|r| r.colo.as_ref())
                .fold(HashMap::new(), |mut acc, colo| {
                    *acc.entry(colo.clone()).or_insert(0) += 1;
                    acc
                })
                .into_iter()
                .max_by_key(|&(_, count)| count)
                .map(|(colo, _)| colo)
                .unwrap_or_else(|| "N/A".to_string());

            target_stats.push(TargetStats {
                url: url.clone(),
                total_checks: entries.len(),
                successful_checks,
                uptime: (successful_checks as f64 / entries.len() as f64) * 100.0,
                rtt_stats,
                unique_colos,
                colo_transitions,
                most_frequent_colo,
            });
        }
    }

    let overall_uptime = if total_checks > 0 {
        (total_successful_checks as f64 / total_checks as f64) * 100.0
    } else {
        0.0
    };

    Report {
        since,
        until,
        configured_targets: target_urls.len(),
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
        report.reported_targets,
        report.configured_targets,
        report.overall_uptime
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
            "  RTT - Min: {}ms, Max: {}ms, Avg: {} (thr: {}ms), Median: {:.2}ms, P95: {} (thr: {}ms)",
            stats.rtt_stats.min,
            stats.rtt_stats.max,
            rtt_avg_colored,
            settings.rtt_threshold_ms,
            stats.rtt_stats.median,
            rtt_p95_colored,
            settings.p95_rtt_threshold_ms
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

pub async fn run_report_once(settings: &Settings, cli: &Cli, client: &Client) -> Result<()> {
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
