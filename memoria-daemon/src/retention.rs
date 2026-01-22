use anyhow::{Context, Result};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};
use rusqlite::OptionalExtension;

use crate::config::Config;
use crate::db;

/// Represents retention policy settings.
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Number of days to retain items.
    pub days: u32,
    /// If true, only delete unstarred items. If false, delete all items regardless of star status.
    pub delete_unstarred_only: bool,
}

impl RetentionPolicy {
    /// Create a retention policy from the config.
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            days: cfg.retention.days,
            delete_unstarred_only: cfg.retention.delete_unstarred_only,
        }
    }

    /// Return the cutoff timestamp (items older than this will be deleted).
    pub fn cutoff_timestamp(&self) -> Result<i64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time error")?
            .as_secs() as i64;

        let retention_seconds = (self.days as i64) * 86400;
        Ok(now - retention_seconds)
    }
}

/// Run the cleanup operation once.
///
/// This function:
/// 1. Queries items older than the retention cutoff
/// 2. Filters out starred items if `delete_unstarred_only` is set
/// 3. Deletes each item and its associated images
/// 4. Removes image files from disk
/// 5. Logs the result
pub async fn run_cleanup(
    conn: std::sync::Arc<Mutex<rusqlite::Connection>>,
    policy: RetentionPolicy,
) -> Result<()> {
    let cutoff = policy.cutoff_timestamp()?;

    let conn_guard = conn.lock().map_err(|e| anyhow::anyhow!("lock poisoned: {}", e))?;

    // Build the deletion query based on policy.
    let query = if policy.delete_unstarred_only {
        "SELECT id FROM items WHERE created_at < ? AND starred = 0"
    } else {
        "SELECT id FROM items WHERE created_at < ?"
    };

    // Find all items to delete.
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

    // For each item, delete associated images and files.
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

/// Delete an item and all associated images (both DB records and files).
pub fn delete_item_and_files(
    conn: &rusqlite::Connection,
    item_id: i64,
) -> Result<()> {
    // Query image files for this item.
    let mut stmt = conn
        .prepare("SELECT id FROM images WHERE item_id = ?")
        .context("failed to prepare image query")?;

    let _image_ids: Vec<i64> = stmt
        .query_map([item_id], |row| row.get(0))
        .context("failed to query images")?
        .collect::<std::result::Result<Vec<i64>, _>>()
        .context("failed to collect image IDs")?;

    // Get the hash for file cleanup.
    let hash: Option<String> = conn
        .query_row(
            "SELECT hash FROM items WHERE id = ?",
            [item_id],
            |row| row.get(0),
        )
        .optional()
        .context("failed to query item hash")?;

    // Delete from database (images table will cascade delete via FK).
    conn.execute("DELETE FROM items WHERE id = ?", [item_id])
        .context("failed to delete item")?;

    // Clean up files if we have a hash.
    if let Some(hash) = hash {
        delete_image_files(&hash)?;
    }

    Ok(())
}

/// Delete image files associated with a hash.
///
/// Safe deletion: ignores missing files.
fn delete_image_files(hash: &str) -> Result<()> {
    let data_dir = db::default_data_dir()?;

    // Delete original image (with any extension).
    let originals_dir = data_dir.join("images/originals");
    if originals_dir.exists() {
        // Find and delete original with any extension.
        if let Ok(entries) = std::fs::read_dir(&originals_dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        let filename = entry.file_name();
                        if let Some(name) = filename.to_str() {
                            // Match filename starting with hash
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

    // Delete thumbnail (always PNG).
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

/// Start the background cleanup scheduler.
///
/// This spawns a Tokio task that:
/// 1. Runs cleanup immediately on startup
/// 2. Then runs cleanup every 24 hours
/// 3. Logs all cleanup operations
pub async fn start_cleanup_scheduler(
    conn: std::sync::Arc<Mutex<rusqlite::Connection>>,
    policy: RetentionPolicy,
) {
    tokio::spawn(async move {
        // Run cleanup immediately on startup.
        info!("running initial cleanup");
        if let Err(err) = run_cleanup(conn.clone(), policy.clone()).await {
            warn!(error=%err, "initial cleanup failed");
        }

        // Schedule recurring cleanup every 24 hours.
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
