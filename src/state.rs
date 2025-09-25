use std::collections::HashMap;
use std::fs::{File as StdFile, OpenOptions};
use std::io::{BufReader, Write};

use anyhow::Result;
use scopeguard::guard;
use tokio::task;

use crate::models::LastSuccessState;

pub(crate) async fn save_last_success_states(states: &[LastSuccessState]) -> Result<()> {
    let state_dir = "state".to_string();
    let state_file = format!("{}/last_success.json", state_dir);
    let states = states.to_vec();

    task::spawn_blocking(move || -> Result<()> {
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
        let cleanup = guard(tmp_file.clone(), |path| {
            let _ = std::fs::remove_file(path);
        });
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
        scopeguard::ScopeGuard::into_inner(cleanup);
        if let Ok(dir) = StdFile::open(&state_dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })
    .await??;

    Ok(())
}

pub(crate) async fn load_last_success_states() -> Result<Vec<LastSuccessState>> {
    let state_file = "state/last_success.json";

    task::spawn_blocking(move || -> Result<Vec<LastSuccessState>> {
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
