use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use tracing::{debug, error, info, warn};
use image::GenericImageView;
use rusqlite::OptionalExtension;

use crate::db;

/// Represents a clipboard entry detected by wl-paste.
#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    /// MIME type (e.g., "text/plain", "image/png")
    pub mime: String,
    /// Raw bytes of the clipboard content
    pub data: Vec<u8>,
    /// Computed SHA-256 hash as hex string
    pub hash: String,
}

impl ClipboardEntry {
    /// Create a new clipboard entry from raw data and MIME type.
    pub fn new(mime: String, data: Vec<u8>) -> Self {
        let hash = compute_hash(&data);
        Self { mime, data, hash }
    }

    /// Returns whether this entry is an image (MIME starts with "image/").
    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

    /// Extract file extension from MIME type (e.g., "png" from "image/png").
    pub fn mime_to_ext(&self) -> &str {
        self.mime
            .split('/')
            .nth(1)
            .unwrap_or("bin")
            .split(';')
            .next()
            .unwrap_or("bin")
    }
}

/// Compute SHA-256 hash of data and return as hex string.
fn compute_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Start the clipboard watcher.
///
/// This spawns a background task that uses polling to detect clipboard changes:
/// 1. Polls clipboard every 300ms via wl-paste (no --watch)
/// 2. Computes SHA-256 hash of content
/// 3. Detects changes by comparing hashes
/// 4. Processes new clipboard entries
/// 5. Handles both text and images
/// 6. Auto-recovers from transient errors
///
/// This approach is robust because:
/// - No process churn or exec weirdness
/// - Works everywhere wl-paste works
/// - Handles race conditions via deduplication
/// - Graceful recovery from failures
pub async fn start_watcher(conn: Arc<Mutex<rusqlite::Connection>>, cfg: crate::config::Config) {
    tokio::spawn(async move {
        // Check prerequisites before starting
        if let Err(e) = check_prerequisites().await {
            error!("FATAL: {}", e);
            error!("Clipboard monitoring disabled");
            return;
        }

        info!("clipboard watcher started (polling every 300ms)");

        let mut last_text_hash: Option<String> = None;
        let mut last_image_hash: Option<String> = None;
        let poll_interval = Duration::from_millis(300);

        loop {
            tokio::time::sleep(poll_interval).await;

            // Poll text clipboard
            match poll_clipboard("text/plain").await {
                Ok(data) if !data.is_empty() => {
                    let hash = compute_hash(&data);
                    if last_text_hash.as_ref() != Some(&hash) {
                        debug!(hash=%hash, "text clipboard changed");
                        last_text_hash = Some(hash.clone());

                        let entry = ClipboardEntry::new("text/plain".to_string(), data);
                        if let Err(err) = process_entry(&conn, entry, cfg.behavior.dedupe).await {
                            warn!(error=%err, "failed to process text clipboard entry");
                        }
                    }
                }
                Ok(_) => {
                    // Empty clipboard, reset hash
                    last_text_hash = None;
                }
                Err(err) => {
                    debug!(error=%err, "failed to poll text clipboard");
                }
            }

            // Poll image clipboard
            match poll_clipboard("image/png").await {
                Ok(data) if !data.is_empty() => {
                    let hash = compute_hash(&data);
                    if last_image_hash.as_ref() != Some(&hash) {
                        debug!(hash=%hash, "image clipboard changed");
                        last_image_hash = Some(hash.clone());

                        let entry = ClipboardEntry::new("image/png".to_string(), data);
                        if let Err(err) = process_entry(&conn, entry, cfg.behavior.dedupe).await {
                            warn!(error=%err, "failed to process image clipboard entry");
                        }
                    }
                }
                Ok(_) => {
                    // Empty clipboard, reset hash
                    last_image_hash = None;
                }
                Err(err) => {
                    debug!(error=%err, "failed to poll image clipboard");
                }
            }
        }
    });
}

/// Check prerequisites for clipboard monitoring.
async fn check_prerequisites() -> Result<()> {
    // Check if wl-paste is available
    match tokio::process::Command::new("which")
        .arg("wl-paste")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {}
        _ => return Err(anyhow::anyhow!("wl-paste not found in PATH - install wl-clipboard package")),
    }

    // Check if Wayland is available
    if std::env::var("WAYLAND_DISPLAY").is_err() {
        return Err(anyhow::anyhow!("WAYLAND_DISPLAY not set - not running under Wayland"));
    }

    Ok(())
}

/// Poll clipboard for a specific MIME type.
/// Returns the clipboard data if available and non-empty, otherwise empty vec.
async fn poll_clipboard(mime: &str) -> Result<Vec<u8>> {
    let output = tokio::process::Command::new("wl-paste")
        .arg("-t")
        .arg(mime)
        .output()
        .await
        .context(format!("failed to run wl-paste for {}", mime))?;

    if !output.status.success() {
        // Status error is not fatal - just no clipboard data for this type
        return Ok(Vec::new());
    }

    Ok(output.stdout)
}

/// Main clipboard watcher loop (removed - using polling instead).
/// See start_watcher() for polling-based implementation.

/// Process a clipboard entry: check for duplicates if enabled, insert or update.
async fn process_entry(
    conn: &Arc<Mutex<rusqlite::Connection>>,
    entry: ClipboardEntry,
    dedupe_enabled: bool,
) -> Result<()> {
    let conn_guard = conn.lock().unwrap();

    // Check if this hash already exists (only if dedupe is enabled).
    let existing_id: Option<i64> = if dedupe_enabled {
        conn_guard
            .query_row(
                "SELECT id FROM items WHERE hash = ?",
                [&entry.hash],
                |row| row.get(0),
            )
            .optional()
            .context("failed to query items by hash")?
    } else {
        None
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time error")?
        .as_secs() as i64;

    if let Some(id) = existing_id {
        // Dedup: update last_used.
        info!(hash=%entry.hash, id=%id, dedupe_enabled=true, "duplicate detected, updating last_used");

        conn_guard
            .execute(
                "UPDATE items SET last_used = ? WHERE id = ?",
                rusqlite::params![now, id],
            )
            .context("failed to update last_used")?;
    } else {
        // New entry: insert.
        let created_at = now;
        let updated_at = now;
        let last_used = now;

        // For images, extract and save files; for text, use empty body initially.
        if entry.is_image() {
            handle_image_insert(&conn_guard, &entry, created_at, updated_at, last_used)?;
        } else {
            // Text entry.
            let title = extract_text_title(&entry.data);
            let body = String::from_utf8_lossy(&entry.data).to_string();

            conn_guard
                .execute(
                    "INSERT INTO items (created_at, updated_at, last_used, title, body, hash) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![created_at, updated_at, last_used, title, body, entry.hash],
                )
                .context("failed to insert text item")?;

            info!(hash=%entry.hash, "inserted text item");
        }
    }

    Ok(())
}

/// Handle image insert: save originals and thumbnails, insert into images table.
fn handle_image_insert(
    conn: &rusqlite::Connection,
    entry: &ClipboardEntry,
    created_at: i64,
    updated_at: i64,
    last_used: i64,
) -> Result<()> {
    let ext = entry.mime_to_ext();

    // Create image directories if needed.
    let originals_dir = db::default_data_dir()?.join("images/originals");
    let thumbs_dir = db::default_data_dir()?.join("images/thumbs");

    std::fs::create_dir_all(&originals_dir)
        .context("failed to create originals directory")?;
    std::fs::create_dir_all(&thumbs_dir).context("failed to create thumbs directory")?;

    // Save original image.
    let original_path = originals_dir.join(format!("{}.{}", entry.hash, ext));
    std::fs::write(&original_path, &entry.data)
        .context("failed to write original image")?;

    debug!(path=%original_path.display(), hash=%entry.hash, "saved original image");

    // Generate thumbnail.
    let thumbnail_path = thumbs_dir.join(format!("{}.png", entry.hash));
    generate_thumbnail(&entry.data, &thumbnail_path)?;

    debug!(path=%thumbnail_path.display(), hash=%entry.hash, "generated thumbnail");

    // Insert into items table.
    conn.execute(
        "INSERT INTO items (created_at, updated_at, last_used, title, body, hash) \
         VALUES (?, ?, ?, ?, ?, ?)",
        rusqlite::params![created_at, updated_at, last_used, format!("Image: {}", entry.hash), "", entry.hash],
    )
    .context("failed to insert image item")?;

    let item_id: i64 = conn
        .query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
        .context("failed to get inserted item ID")?;

    // Insert into images table.
    conn.execute(
        "INSERT INTO images (item_id, created_at, mime, bytes) VALUES (?, ?, ?, ?)",
        rusqlite::params![item_id, created_at, entry.mime, entry.data.as_slice()],
    )
    .context("failed to insert into images table")?;

    info!(
        hash=%entry.hash,
        id=%item_id,
        original=%original_path.display(),
        thumbnail=%thumbnail_path.display(),
        "inserted image item with thumbnail"
    );

    Ok(())
}

/// Generate a thumbnail from image data (max 256x256, aspect ratio preserved).
fn generate_thumbnail(image_data: &[u8], output_path: &Path) -> Result<()> {
    // Decode the image.
    let img = image::load_from_memory(image_data)
        .context("failed to decode image")?;

    // Resize to fit within 256x256, preserving aspect ratio.
    let max_size = 256u32;
    let (w, h) = img.dimensions();

    let (new_w, new_h) = if w > h {
        let resized_w = w.min(max_size);
        let resized_h = (h as f32 * (resized_w as f32 / w as f32)) as u32;
        (resized_w, resized_h)
    } else {
        let resized_h = h.min(max_size);
        let resized_w = (w as f32 * (resized_h as f32 / h as f32)) as u32;
        (resized_w, resized_h)
    };

    let thumbnail = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);

    // Save as PNG.
    thumbnail
        .save_with_format(output_path, image::ImageFormat::Png)
        .context("failed to save thumbnail")?;

    Ok(())
}

/// Extract a short title from text content (first line, max 100 chars).
fn extract_text_title(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(100)
        .collect()
}
