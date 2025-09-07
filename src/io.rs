use crate::models::{CheckResult, LastSuccessState};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fs::{File as StdFile, OpenOptions};
use std::io::{BufRead, BufReader, Write};

pub async fn write_results(path: String, format: String, results: Vec<CheckResult>) -> Result<()> {
    if format == "none" {
        return Ok(());
    }

    tokio::task::spawn_blocking(move || -> Result<()> {
        if let Some(parent) = std::path::Path::new(&path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        match format.as_str() {
            "json" | "jsonl" => {
                let mut file = std::io::BufWriter::new(file);
                for result in &results {
                    serde_json::to_writer(&mut file, result)?;
                    file.write_all(b"\n")?;
                }
                file.flush()?;
            }
            other => anyhow::bail!("unsupported output_format: {}", other),
        }
        Ok(())
    })
    .await??;
    Ok(())
}

pub async fn load_check_results(
    path: String,
    format: String,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> Result<Vec<CheckResult>> {
    let results = tokio::task::spawn_blocking(move || -> Result<Vec<CheckResult>> {
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

pub async fn save_last_success_states(states: &[LastSuccessState]) -> Result<()> {
    let state_dir = "state".to_string();
    let state_file = format!("{}/last_success.json", state_dir);
    let states = states.to_vec();

    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&state_dir)?;

        let mut all_states: HashMap<String, LastSuccessState> = HashMap::new();

        if let Ok(file) = StdFile::open(&state_file) {
            let reader = BufReader::new(file);
            if let Ok(existing_states) = serde_json::from_reader::<_, Vec<LastSuccessState>>(reader)
            {
                for state in existing_states {
                    all_states.insert(state.url.clone(), state);
                }
            }
        }

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
            let mut writer = std::io::BufWriter::new(&file);
            serde_json::to_writer_pretty(&mut writer, &updated_states)?;
            writer.flush()?;
            file.sync_all()?;
        }
        std::fs::rename(&tmp_file, &state_file)?;
        if let Ok(dir) = StdFile::open(&state_dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })
    .await??;

    Ok(())
}

pub async fn load_last_success_states() -> Result<Vec<LastSuccessState>> {
    let state_file = "state/last_success.json";

    tokio::task::spawn_blocking(move || -> Result<Vec<LastSuccessState>> {
        let file = match StdFile::open(state_file) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };

        let reader = BufReader::new(file);
        let states = match serde_json::from_reader(reader) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to parse last success states, starting fresh: {}", e);
                Vec::new()
            }
        };

        Ok(states)
    })
    .await?
}
