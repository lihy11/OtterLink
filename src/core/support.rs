use std::{path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::{fs, fs::OpenOptions, io::AsyncWriteExt};

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn shorten(s: &str, max_chars: usize) -> String {
    let mut iter = s.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

pub async fn append_jsonl(path: &PathBuf, value: &Value, label: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {label} dir failed: {:?}", parent))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("open {label} failed: {:?}", path))?;
    file.write_all(serde_json::to_string(value)?.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}
