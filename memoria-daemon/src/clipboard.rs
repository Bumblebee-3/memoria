use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncBufReadExt;
use tokio::process::{Child, Command};
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
/// This spawns a background task that:
/// 1. Runs `wl-paste --watch` to monitor clipboard changes
/// 2. Detects MIME types for each entry
/// 3. Computes SHA-256 hashes
/// 4. Stores or deduplicates in the database
/// 5. Handles image thumbnails
/// 6. Auto-restarts on crash
pub async fn start_watcher(conn: Arc<Mutex<rusqlite::Connection>>, cfg: crate::config::Config) {
    tokio::spawn(async move {
        loop {
            if let Err(err) = run_clipboard_watcher(conn.clone(), cfg.clone()).await {
                error!(error=%err, "clipboard watcher crashed, restarting in 5s");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    });
}

/// Main clipboard watcher loop.
///
/// Uses `wl-paste --watch` to detect clipboard changes and process them.
async fn run_clipboard_watcher(
    conn: Arc<Mutex<rusqlite::Connection>>,
    cfg: crate::config::Config,
) -> Result<()> {
    // We use a subprocess approach:
    // `wl-paste --watch` outputs available MIME types on each change,
    // one per line, then a blank line to signal completion.
    let mut child = spawn_wl_paste_watch()?;

    let stdout = child
        .stdout
        .take()
        .context("failed to get stdout from wl-paste")?;

    let reader = tokio::io::BufReader::new(stdout);
    let mut lines = reader.lines();

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();

        // Blank line signals end of MIME list for this clipboard change.
        if line.is_empty() {
            debug!("clipboard change detected, processing");

            // Try to fetch the best available MIME type.
            if let Err(err) = process_clipboard_entry(&conn, &cfg).await {
                warn!(error=%err, "failed to process clipboard entry");
            }
            continue;
        }

        // Lines are MIME type strings; we'll handle them in process_clipboard_entry.
        debug!(mime=%line, "clipboard MIME type available");
    }

    // If we get here, wl-paste exited. Return an error to trigger a restart.
    bail!("wl-paste process exited unexpectedly")
}

/// Spawn the `wl-paste --watch` subprocess.
fn spawn_wl_paste_watch() -> Result<Child> {
    Command::new("wl-paste")
        .arg("--watch")
        .arg("echo")
        .arg("")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn wl-paste --watch")
}

/// Process a clipboard entry by fetching available MIME types and the best content.
async fn process_clipboard_entry(
    conn: &Arc<Mutex<rusqlite::Connection>>,
    cfg: &crate::config::Config,
) -> Result<()> {
    // Query available MIME types.
    let mimes = query_available_mimes().await?;
    if mimes.is_empty() {
        debug!("no MIME types available in clipboard");
        return Ok(());
    }

    debug!(mime_types=?mimes, "available MIME types");

    // Prefer image, then text/plain, then first available.
    let preferred_mime = choose_best_mime(&mimes);
    debug!(selected_mime=%preferred_mime, "selected MIME type");

    // Fetch clipboard content.
    let data = fetch_clipboard_data(&preferred_mime).await?;
    if data.is_empty() {
        debug!(mime=%preferred_mime, "clipboard content is empty");
        return Ok(());
    }

    let entry = ClipboardEntry::new(preferred_mime, data);
    debug!(hash=%entry.hash, mime=%entry.mime, size=entry.data.len(), "clipboard entry ready");

    // Process the entry: insert or dedupe (if dedupe enabled).
    process_entry(conn, entry, cfg.behavior.dedupe).await?;

    Ok(())
}

/// Query available MIME types using `wl-paste -l`.
async fn query_available_mimes() -> Result<Vec<String>> {
    let output = tokio::process::Command::new("wl-paste")
        .arg("-l")
        .output()
        .await
        .context("failed to run wl-paste -l")?;

    if !output.status.success() {
        bail!("wl-paste -l failed");
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mimes: Vec<String> = text.lines().map(|s| s.to_string()).collect();

    Ok(mimes)
}

/// Choose the best MIME type from available options.
/// Prioritizes images, then text/plain, then the first available.
fn choose_best_mime(mimes: &[String]) -> String {
    // Prefer image types.
    for mime in mimes {
        if mime.starts_with("image/") {
            return mime.clone();
        }
    }

    // Then prefer text/plain.
    for mime in mimes {
        if mime == "text/plain" {
            return mime.clone();
        }
    }

    // Fall back to first available.
    mimes.first().cloned().unwrap_or_else(|| "text/plain".to_string())
}

/// Fetch clipboard content for a specific MIME type.
async fn fetch_clipboard_data(mime: &str) -> Result<Vec<u8>> {
    let output = tokio::process::Command::new("wl-paste")
        .arg("-t")
        .arg(mime)
        .output()
        .await
        .context("failed to run wl-paste")?;

    if !output.status.success() {
        bail!("wl-paste failed for MIME type: {}", mime);
    }

    Ok(output.stdout)
}

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
