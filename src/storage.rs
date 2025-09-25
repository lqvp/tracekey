use std::fs::{File as StdFile, OpenOptions};
use std::io::{BufRead, BufReader, Write};

use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::task;

use crate::models::CheckResult;

pub(crate) async fn write_results(
    path: String,
    format: String,
    results: Vec<CheckResult>,
) -> Result<()> {
    if format == "none" {
        return Ok(());
    }

    task::spawn_blocking(move || -> Result<()> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        match format.as_str() {
            "json" | "jsonl" => {
                let mut file = std::io::BufWriter::new(file);
                for result in &results {
                    serde_json::to_writer(&mut file, result)?;
                    file.write_all(b"\n")?;
                }
                file.flush()?;
                file.get_ref().sync_all()?;
            }
            other => anyhow::bail!("unsupported output_format: {}", other),
        }
        Ok(())
    })
    .await??;
    Ok(())
}

pub(crate) async fn load_check_results(
    path: String,
    format: String,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<Vec<CheckResult>> {
    let results = task::spawn_blocking(move || -> Result<Vec<CheckResult>> {
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
                            if e.is_eof() {
                                break;
                            }
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
