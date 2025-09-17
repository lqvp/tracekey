use crate::config::Settings;
use crate::io::{load_last_success_states, save_last_success_states, write_results};
use crate::misskey::post_to_misskey;
use crate::models::{CheckResult, LastSuccessState};
use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use futures::stream::StreamExt;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time;
use url::Url;

pub async fn run_checks_once(
    settings: &Settings,
    client: &Client,
    misskey_semaphore: Arc<Semaphore>,
) -> Result<()> {
    println!("Running check...");

    let mut prev_states: HashMap<String, LastSuccessState> = match load_last_success_states().await
    {
        Ok(states) => states,
        Err(e) => {
            eprintln!("Failed to load previous success states: {}", e);
            Vec::new()
        }
    }
    .into_iter()
    .map(|state| (state.url.clone(), state))
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
                    result.rtt_millis.unwrap_or(0),
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
    // Colo変更検知とMisskey投稿
    let mut colo_change_messages = Vec::new();
    for result in &results {
        if result.success {
            if let Some(prev_state) = prev_states.get_mut(&result.url) {
                if let (Some(curr_colo), Some(prev_colo)) =
                    (result.colo.as_ref(), prev_state.colo.as_ref())
                {
                    if curr_colo != prev_colo {
                        let now = Utc::now();
                        if now - prev_state.last_notification_timestamp > ChronoDuration::minutes(5)
                        {
                            let domain = if let Ok(parsed_url) = result.url.parse::<url::Url>() {
                                parsed_url.host_str().unwrap_or(&result.url).to_string()
                            } else {
                                result.url.clone()
                            };
                            let (rtt_color, rtt_text, rtt_unit): (&str, String, &str) =
                                match result.rtt_millis {
                                    Some(ms @ 0..=299) => ("3a3", ms.to_string(), "ms"), // green
                                    Some(ms @ 300..=499) => ("991", ms.to_string(), "ms"), // yellow
                                    Some(ms @ 500..=999) => ("c52", ms.to_string(), "ms"), // orange
                                    Some(ms) => ("b22", ms.to_string(), "ms"),           // red
                                    None => ("999", "N/A".into(), ""), // gray for no data
                                };
                            let message = format!(
                                "<small>`{}`</small>→`{}` $[border.color=0000,radius=10 $[bg.color={} $[fg.color=fff  {}<small>{}</small> ]]] ?[{}]({})",
                                prev_colo,
                                curr_colo,
                                rtt_color,
                                rtt_text,
                                rtt_unit,
                                domain,
                                result.url
                            );
                            colo_change_messages.push(message);
                            prev_state.last_notification_timestamp = now;
                        }
                    }
                }
            }
        }
    }

    if !colo_change_messages.is_empty() && settings.colo_change_notify_misskey {
        if let Some(token) = &settings.misskey_token {
            if !token.is_empty() {
                let message = colo_change_messages.join("\n");
                let misskey_client = client.clone();
                let misskey_url = settings.misskey_url.clone();
                let misskey_token = token.clone();
                let misskey_visibility = settings.reporting.misskey_visibility.clone();
                let sem_clone = misskey_semaphore.clone();

                tokio::spawn(async move {
                    let permit = match sem_clone.acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => {
                            eprintln!(
                                "Misskey notification semaphore closed, skipping notification."
                            );
                            return;
                        }
                    };
                    let _permit = permit;
                    println!("Posting colo change to Misskey...");
                    match post_to_misskey(
                        &misskey_client,
                        &misskey_url,
                        &misskey_token,
                        &message,
                        &misskey_visibility,
                    )
                    .await
                    {
                        Ok(_) => println!("Colo change posted to Misskey successfully."),
                        Err(e) => eprintln!("Failed to post colo change to Misskey: {}", e),
                    }
                });
            }
        }
    }

    // 最後の成功状態を更新
    let success_states: Vec<LastSuccessState> = results
        .iter()
        .filter(|r| r.success)
        .map(|r| {
            let last_notification_timestamp = prev_states
                .get(&r.url)
                .map(|s| s.last_notification_timestamp)
                .unwrap_or_else(Utc::now);
            LastSuccessState {
                url: r.url.clone(),
                colo: r.colo.clone(),
                timestamp: r.timestamp,
                last_notification_timestamp,
            }
        })
        .collect();

    if !success_states.is_empty() {
        if let Err(e) = save_last_success_states(&success_states).await {
            eprintln!("Failed to save last success states: {}", e);
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

async fn get_cloudflare_trace(client: &Client, url: &str) -> Result<CheckResult> {
    let base_url = Url::parse(url)?;
    let trace_url = base_url.join("/cdn-cgi/trace")?.to_string();
    let start_time = time::Instant::now();
    let resp = client
        .get(&trace_url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let rtt = start_time.elapsed();

    const COLO_PREFIX: &str = "colo=";
    let colo_opt = resp
        .lines()
        .find_map(|line| line.strip_prefix(COLO_PREFIX))
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
