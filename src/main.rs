use anyhow::Result;
use config::{Config, File};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time;

#[derive(Debug, Deserialize)]
struct Settings {
    misskey_url: String,
    misskey_token: String,
    misskey_visibility: String,
    target_urls: Vec<String>,
    check_interval_seconds: u64,
    user_agent: String,
    request_timeout_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let settings = load_settings()?;
    let mut last_colos: HashMap<String, String> = HashMap::new();

    let client = Client::builder()
        .user_agent(&settings.user_agent)
        .timeout(Duration::from_secs(settings.request_timeout_seconds))
        .build()?;

    println!("Starting tracekey monitoring with User-Agent: {}", settings.user_agent);

    loop {
        println!("Running check...");
        let mut changes: Vec<String> = Vec::new();

        for url in &settings.target_urls {
            println!("Checking trace for {}", url);
            match get_cloudflare_trace(&client, url).await {
                Ok(trace) => {
                    if let Some(colo) = trace.get("colo") {
                        println!("Current colo for {}: {}", url, colo);
                        let last_colo = last_colos.entry(url.clone()).or_insert_with(|| colo.clone());
                        if !last_colo.is_empty() && last_colo != colo {
                            let message = format!(
                                "Cloudflare colocation for {} changed: `{}` -> `{}`",
                                url, last_colo, colo
                            );
                            println!("CHANGE DETECTED: {}", message);
                            changes.push(message);
                        }
                        *last_colo = colo.to_string();
                    } else {
                        eprintln!("'colo' not found in trace for {}", url);
                    }
                }
                Err(e) => eprintln!("Failed to get trace for {}: {}", url, e),
            }
        }

        if !changes.is_empty() {
            let final_message = changes.join("\n");
            println!("Posting to Misskey:\n{}", final_message);
            if let Err(e) =
                post_to_misskey(&client, &settings.misskey_url, &settings.misskey_token, &final_message, &settings.misskey_visibility)
                    .await
            {
                eprintln!("Failed to post to Misskey: {}", e);
            }
        }

        println!("Check finished. Waiting for {} seconds.", settings.check_interval_seconds);
        time::sleep(Duration::from_secs(settings.check_interval_seconds)).await;
    }
}

fn load_settings() -> Result<Settings> {
    let settings = Config::builder()
        .add_source(File::with_name("config/default"))
        .build()?;
    Ok(settings.try_deserialize()?)
}

async fn get_cloudflare_trace(client: &Client, url: &str) -> Result<HashMap<String, String>> {
    let trace_url = format!("{}/cdn-cgi/trace", url);
    let resp = client.get(&trace_url).send().await?.text().await?;

    let mut trace_data = HashMap::new();
    for line in resp.lines() {
        if let Some((key, value)) = line.split_once('=') {
            trace_data.insert(key.to_string(), value.to_string());
        }
    }
    Ok(trace_data)
}

async fn post_to_misskey(client: &Client, url: &str, token: &str, text: &str, visibility: &str) -> Result<()> {
    let api_url = format!("{}/api/notes/create", url);

    let mut params = HashMap::new();
    params.insert("i", token);
    params.insert("text", text);
    params.insert("visibility", visibility);

    client.post(&api_url).json(&params).send().await?;
    Ok(())
}
