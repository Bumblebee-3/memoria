use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH, Duration};
use tracing::{debug, error, info, warn};
use image::GenericImageView;
use rusqlite::OptionalExtension;

use crate::db;

#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub mime: String,
    pub data: Vec<u8>,
    pub hash: String,
}

impl ClipboardEntry {
    pub fn new(mime: String, data: Vec<u8>) -> Self {
        let hash = compute_hash(&data);
        Self { mime, data, hash }
    }

    pub fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }

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

fn compute_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub async fn start_watcher(conn: Arc<Mutex<rusqlite::Connection>>, cfg: crate::config::Config) {
    tokio::spawn(async move {
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
                    last_text_hash = None;
                }
                Err(err) => {
                    debug!(error=%err, "failed to poll text clipboard");
                }
            }

            if let Some((mime, data)) = poll_image_clipboard().await {
                let hash = compute_hash(&data);
                if last_image_hash.as_ref() != Some(&hash) {
                    debug!(hash=%hash, mime=%mime, "image clipboard changed");
                    last_image_hash = Some(hash.clone());

                    let entry = ClipboardEntry::new(mime, data);
                    if let Err(err) = process_entry(&conn, entry, cfg.behavior.dedupe).await {
                        warn!(error=%err, "failed to process image clipboard entry");
                    }
                }
            } else {
                last_image_hash = None;
            }
        }
    });
}

async fn check_prerequisites() -> Result<()> {
    match tokio::process::Command::new("which")
        .arg("wl-paste")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {}
        _ => return Err(anyhow::anyhow!("wl-paste not found in PATH - install wl-clipboard package")),
    }

    if std::env::var("WAYLAND_DISPLAY").is_err() {
        return Err(anyhow::anyhow!("WAYLAND_DISPLAY not set - not running under Wayland"));
    }

    Ok(())
}

async fn poll_clipboard(mime_type: &str) -> Result<Vec<u8>> {
    let output = tokio::process::Command::new("wl-paste")
        .arg("--type")
        .arg(mime_type)
        .output()
        .await
        .context(format!("failed to run wl-paste for {}", mime_type))?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Ok(Vec::new())
    }
}

async fn poll_image_clipboard() -> Option<(String, Vec<u8>)> {
    let mimes = ["image/png", "image/jpeg", "image/webp", "image/bmp"];
    
    for mime in &mimes {
        match poll_clipboard(mime).await {
            Ok(data) if !data.is_empty() => {
                return Some((mime.to_string(), data));
            }
            _ => {}
        }
    }
    None
}
async fn process_entry(
    conn: &Arc<Mutex<rusqlite::Connection>>,
    entry: ClipboardEntry,
    dedupe_enabled: bool,
) -> Result<()> {
    let conn_clone = conn.clone();

    tokio::task::spawn_blocking(move || {
        let conn_guard = conn_clone.lock().unwrap();

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
            info!(hash=%entry.hash, id=%id, dedupe_enabled=true, "duplicate detected, updating last_used");

            conn_guard
                .execute(
                    "UPDATE items SET last_used = ? WHERE id = ?",
                    rusqlite::params![now, id],
                )
                .context("failed to update last_used")?;
        } else {
            let created_at = now;
            let updated_at = now;
            let last_used = now;

            if entry.is_image() {
                handle_image_insert(&conn_guard, &entry, created_at, updated_at, last_used)?;
            } else {
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
    })
    .await
    .context("spawn_blocking task panicked")?
}

fn handle_image_insert(
    conn: &rusqlite::Connection,
    entry: &ClipboardEntry,
    created_at: i64,
    updated_at: i64,
    last_used: i64,
) -> Result<()> {
    let ext = entry.mime_to_ext();

    let originals_dir = db::default_data_dir()?.join("images/originals");
    let thumbs_dir = db::default_data_dir()?.join("images/thumbs");

    std::fs::create_dir_all(&originals_dir)
        .context("failed to create originals directory")?;
    std::fs::create_dir_all(&thumbs_dir).context("failed to create thumbs directory")?;

    let original_path = originals_dir.join(format!("{}.{}", entry.hash, ext));
    std::fs::write(&original_path, &entry.data)
        .context("failed to write original image")?;

    debug!(path=%original_path.display(), hash=%entry.hash, "saved original image");

    let thumbnail_path = thumbs_dir.join(format!("{}.png", entry.hash));
    generate_thumbnail(&entry.data, &thumbnail_path)?;

    debug!(path=%thumbnail_path.display(), hash=%entry.hash, "generated thumbnail");

    conn.execute(
        "INSERT INTO items (created_at, updated_at, last_used, title, body, hash) \
         VALUES (?, ?, ?, ?, ?, ?)",
        rusqlite::params![created_at, updated_at, last_used, format!("Image: {}", entry.hash), "", entry.hash],
    )
    .context("failed to insert image item")?;

    let item_id: i64 = conn
        .query_row("SELECT last_insert_rowid()", [], |row| row.get(0))
        .context("failed to get inserted item ID")?;

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

fn generate_thumbnail(image_data: &[u8], output_path: &Path) -> Result<()> {
    let img = image::load_from_memory(image_data)
        .context("failed to decode image")?;

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

    thumbnail
        .save_with_format(output_path, image::ImageFormat::Png)
        .context("failed to save thumbnail")?;

    Ok(())
}

fn extract_text_title(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(100)
        .collect()
}
