use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Local, Utc};
use colored::*;
use reqwest::Client;
use statistical::mean;

use crate::cli::Cli;
use crate::config::{ReportingSettings, Settings};
use crate::misskey::post_to_misskey;
use crate::models::{CheckResult, Report, RttStats, TargetStats};
use crate::storage::load_check_results;

pub(crate) async fn run_report_once(settings: &Settings, cli: &Cli, client: &Client) -> Result<()> {
    let until = cli.until.unwrap_or_else(Utc::now);
    let since = if let Some(s) = cli.since {
        s
    } else {
        let duration_chrono = ChronoDuration::from_std(settings.reporting.interval)
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
                median: percentile(&sorted_rtts, 0.5),
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

        let mut colo_frequency = HashMap::new();
        for r in &target_results {
            if let Some(ref colo) = r.colo {
                *colo_frequency.entry(colo.clone()).or_insert(0) += 1;
            }
        }

        let most_frequent_colo = colo_frequency
            .iter()
            .max_by(|(ca, a), (cb, b)| a.cmp(b).then_with(|| ca.cmp(cb)))
            .map(|(colo, _)| colo.clone())
            .unwrap_or_default();

        let mut unique_colos_list: Vec<_> = colo_frequency.keys().cloned().collect();
        unique_colos_list.sort();

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
