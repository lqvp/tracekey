use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use rand::{Rng, rng};
use reqwest::Client;
use tokio::time;
use url::Url;

pub(crate) async fn post_to_misskey(
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
                if status.is_client_error() {
                    return Err(anyhow::anyhow!(
                        "Misskey API client error {} - {}",
                        status,
                        error_text
                    ));
                }
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
        let jitter_ms: u64 = rng().random_range(0u64..1000u64);
        delay = delay
            .saturating_mul(2)
            .saturating_add(Duration::from_millis(jitter_ms));
    }
}
