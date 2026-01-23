use anyhow::{Context, Result};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use rusqlite::OptionalExtension;

use crate::config::Config;
use crate::db;

#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    pub days: u32,
    pub delete_unstarred_only: bool,
}

impl RetentionPolicy {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            days: cfg.retention.days,
            delete_unstarred_only: cfg.retention.delete_unstarred_only,
        }
    }

    pub fn cutoff_timestamp(&self) -> Result<i64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time error")?
            .as_secs() as i64;

        let retention_seconds = (self.days as i64) * 86400;
        Ok(now - retention_seconds)
    }
}

pub async fn run_cleanup(
    conn: std::sync::Arc<Mutex<rusqlite::Connection>>,
    policy: RetentionPolicy,
) -> Result<()> {
    let cutoff = policy.cutoff_timestamp()?;

    let conn_guard = conn.lock().map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;

    let query = if policy.delete_unstarred_only {
        "SELECT id FROM items WHERE created_at < ? AND starred = 0"
    } else {
        "SELECT id FROM items WHERE created_at < ?"
    };

    let mut stmt = conn_guard
        .prepare(query)
        .context("failed to prepare deletion query")?;

    let item_ids: Vec<i64> = stmt
        .query_map([cutoff], |row| row.get(0))
        .context("failed to query items for deletion")?
        .collect::<std::result::Result<Vec<i64>, _>>()
        .context("failed to collect item IDs")?;

    if item_ids.is_empty() {
        info!("cleanup: no items to delete");
        return Ok(());
    }

    for item_id in &item_ids {
        if let Err(err) = delete_item_and_files(&conn_guard, *item_id) {
            warn!(item_id, error=%err, "failed to delete item");
        }
    }

    let deleted_count = item_ids.len();
    info!(
        deleted_count,
        retention_days = policy.days,
        delete_unstarred_only = policy.delete_unstarred_only,
        "cleanup run completed"
    );

    Ok(())
}

pub fn delete_item_and_files(
    conn: &rusqlite::Connection,
    item_id: i64,
) -> Result<()> {
    let mut stmt = conn
        .prepare("SELECT id FROM images WHERE item_id = ?")
        .context("failed to prepare image query")?;

    let _image_ids: Vec<i64> = stmt
        .query_map([item_id], |row| row.get(0))
        .context("failed to query images")?
        .collect::<std::result::Result<Vec<i64>, _>>()
        .context("failed to collect image IDs")?;

    let hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM items WHERE id = ?",
            [item_id],
            |row| row.get(0),
        )
        .optional()
        .context("failed to query item hash")?;

    conn.execute("DELETE FROM items WHERE id = ?", [item_id])
        .context("failed to delete item")?;

    if let Some(hash) = hash {
        delete_image_files(&hash)?;
    }

    Ok(())
}
fn delete_image_files(hash: &str) -> Result<()> {
    let data_dir = db::default_data_dir()?;

    let originals_dir = data_dir.join("images/originals");
    if originals_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&originals_dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        let filename = entry.file_name();
                        if let Some(name) = filename.to_str() {
                            if name.starts_with(hash) && name.contains('.') {
                                if let Err(e) = std::fs::remove_file(entry.path()) {
                                    if e.kind() != std::io::ErrorKind::NotFound {
                                        warn!(
                                            path=%entry.path().display(),
                                            error=%e,
                                            "failed to delete original image"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let thumbnail_path = data_dir.join(format!("images/thumbs/{}.png", hash));
    if let Err(err) = std::fs::remove_file(&thumbnail_path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn!(
                path=%thumbnail_path.display(),
                error=%err,
                "failed to delete thumbnail"
            );
        }
    }

    Ok(())
}

pub async fn start_cleanup_scheduler(
    conn: std::sync::Arc<Mutex<rusqlite::Connection>>,
    policy: RetentionPolicy,
) {
    tokio::spawn(async move {
        info!("running initial cleanup");
        if let Err(err) = run_cleanup(conn.clone(), policy.clone()).await {
            warn!(error=%err, "initial cleanup failed");
        }

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(86400));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            info!("running scheduled cleanup");
            if let Err(err) = run_cleanup(conn.clone(), policy.clone()).await {
                warn!(error=%err, "scheduled cleanup failed");
            }
        }
    });
}
