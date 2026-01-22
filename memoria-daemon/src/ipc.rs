use anyhow::{anyhow, Context, Result};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use tracing::{error};


#[derive(Debug)]
pub enum IpcRequest {
    /// List recent items, newest first. Optional: filter by starred only.
    List { limit: Option<u32>, starred_only: bool },
    /// Full-text search.
    Search { query: String, limit: Option<u32> },
    /// Images only gallery.
    Gallery { limit: Option<u32> },
    /// Star/unstar an item.
    Star { id: i64, value: bool },
    /// Restore item to clipboard.
    Copy { id: i64 },

    /// Delete specific items (only non-starred ones; starred items are silently ignored).
    Delete { ids: Vec<i64> },
    /// Delete all non-starred items (and related images).
    DeleteAllExceptStarred,
    /// Delete specific items by ID.
    DeleteItems { ids: Vec<i64> },
    /// Fetch UI, grid, and behavior settings.
    GetSettings,
}

#[derive(Debug, Serialize)]
pub struct IpcResponse<T> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> IpcResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ItemSummary {
    pub id: i64,
    pub title: Option<String>,
    pub body: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_used: Option<i64>,
    pub starred: bool,
    pub hash: Option<String>,
    pub has_image: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_path: Option<String>,
}

/// Handle one accepted Unix domain socket connection.
pub async fn handle_connection(stream: UnixStream, conn: Arc<Mutex<rusqlite::Connection>>, cfg: Arc<crate::config::Config>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }

        let parsed: IpcRequest = match parse_request(&line) {
            Ok(req) => req,
            Err(err) => {
                let _ = writer
                    .write_all(format_json(&IpcResponse::<()>::err(format!("invalid json: {err}"))).as_bytes())
                    .await;
                continue;
            }
        };

        let response = dispatch_request(&conn, &cfg, parsed)
            .await
            .unwrap_or_else(|err| IpcResponse::<serde_json::Value>::err(format!("{err}")));

        if let Err(err) = writer.write_all(format_json(&response).as_bytes()).await {
            error!(error=%err, "failed to write IPC response");
            break;
        }
    }
}

fn format_json<T: Serialize>(resp: &IpcResponse<T>) -> String {
    serde_json::to_string(resp).unwrap_or_else(|e| {
        format!("{{\"ok\":false,\"error\":\"serialization error: {e}\"}}")
    }) + "\n"
}

/// Parse a request flexibly. Accepts either:
/// {"cmd":"list","limit":10}
/// or {"cmd":"list","args":{"limit":10}}
fn parse_request(line: &str) -> Result<IpcRequest> {
    let v: Value = serde_json::from_str(line)?;
    let obj = v
        .as_object()
        .ok_or_else(|| anyhow!("request must be a JSON object"))?;

    let cmd_val = obj
        .get("cmd")
        .ok_or_else(|| anyhow!("missing cmd"))?;
    let cmd = cmd_val
        .as_str()
        .ok_or_else(|| anyhow!("cmd must be a string"))?
        .to_lowercase();

    // Support optional args object.
    let args_obj = obj
        .get("args")
        .and_then(|a| a.as_object());

    // Helper to pull a field from args or top-level.
    let get = |key: &str| -> Option<&Value> {
        args_obj
            .and_then(|m| m.get(key))
            .or_else(|| obj.get(key))
    };

    match cmd.as_str() {
        "list" => {
            let limit = get("limit").and_then(|v| v.as_u64()).map(|n| n as u32);
            let starred_only = get("starred_only").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(IpcRequest::List { limit, starred_only })
        }
        "search" => {
            let query = get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("search requires query"))?
                .to_string();
            let limit = get("limit").and_then(|v| v.as_u64()).map(|n| n as u32);
            Ok(IpcRequest::Search { query, limit })
        }
        "gallery" => {
            let limit = get("limit").and_then(|v| v.as_u64()).map(|n| n as u32);
            Ok(IpcRequest::Gallery { limit })
        }
        "star" => {
            let id = get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| anyhow!("star requires id"))?;
            let value = get("value")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| anyhow!("star requires value"))?;
            Ok(IpcRequest::Star { id, value })
        }
        "copy" => {
            let id = get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| anyhow!("copy requires id"))?;
            Ok(IpcRequest::Copy { id })
        }
        "delete" => {
            let ids_val = get("ids")
                .ok_or_else(|| anyhow!("delete requires ids"))?;
            let ids_array = ids_val
                .as_array()
                .ok_or_else(|| anyhow!("ids must be an array"))?;
            
            if ids_array.is_empty() {
                return Err(anyhow!("ids array cannot be empty"));
            }
            
            let ids: Result<Vec<i64>> = ids_array
                .iter()
                .map(|v| v.as_i64().ok_or_else(|| anyhow!("ids must contain only integers")))
                .collect();
            
            Ok(IpcRequest::Delete { ids: ids? })
        }
        "delete_all_except_starred" => Ok(IpcRequest::DeleteAllExceptStarred),
        "delete_items" => {
            let ids_val = get("ids").ok_or_else(|| anyhow!("delete_items requires ids array"))?;
            let ids_arr = ids_val.as_array().ok_or_else(|| anyhow!("ids must be an array"))?;
            let mut ids: Vec<i64> = Vec::with_capacity(ids_arr.len());
            for v in ids_arr {
                let id = v.as_i64().ok_or_else(|| anyhow!("ids must contain integers"))?;
                ids.push(id);
            }
            Ok(IpcRequest::DeleteItems { ids })
        }
        "get_settings" => Ok(IpcRequest::GetSettings),
        other => Err(anyhow!("unknown cmd: {other}")),
    }
}

async fn dispatch_request(
    conn: &Arc<Mutex<rusqlite::Connection>>,
    cfg: &Arc<crate::config::Config>,
    req: IpcRequest,
) -> Result<IpcResponse<serde_json::Value>> {
    match req {
        IpcRequest::List { limit, starred_only } => {
            let rows = list_items(conn, limit.unwrap_or(50), starred_only).await?;
            Ok(IpcResponse::ok(serde_json::to_value(rows)?))
        }
        IpcRequest::Search { query, limit } => {
            let rows = search_items(conn, &query, limit.unwrap_or(50)).await?;
            Ok(IpcResponse::ok(serde_json::to_value(rows)?))
        }
        IpcRequest::Gallery { limit } => {
            let rows = gallery_items(conn, limit.unwrap_or(50)).await?;
            Ok(IpcResponse::ok(serde_json::to_value(rows)?))
        }
        IpcRequest::Star { id, value } => {
            let updated = star_item(conn, id, value).await?;
            Ok(IpcResponse::ok(serde_json::json!({"updated": updated})))
        }
        IpcRequest::Copy { id } => {
            copy_to_clipboard(conn, id).await?;
            Ok(IpcResponse::ok(serde_json::json!({"copied": true})))
        }
        IpcRequest::Delete { ids } => {
            let deleted = delete_items(conn, ids).await?;
            Ok(IpcResponse::ok(serde_json::json!({"deleted": deleted})))
        }
        IpcRequest::DeleteAllExceptStarred => {
            let result = delete_all_except_starred(conn).await?;
            Ok(IpcResponse::ok(serde_json::json!({
                "deleted_items": result.deleted_items,
                "deleted_images": result.deleted_images
            })))
        }
        IpcRequest::DeleteItems { ids } => {
            let conn = conn.clone();
            let deleted_count = tokio::task::spawn_blocking(move || {
                let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
                let mut count: i64 = 0;
                for id in ids {
                    match crate::retention::delete_item_and_files(&conn, id) {
                        Ok(_) => { count += 1; },
                        Err(err) => {
                            // Log and continue to attempt other deletions
                            tracing::warn!(error=%err, item_id=id, "failed to delete item by id");
                        }
                    }
                }
                Ok::<i64, anyhow::Error>(count)
            }).await??;

            Ok(IpcResponse::ok(serde_json::json!({
                "deleted_count": deleted_count
            })))
        }
        IpcRequest::GetSettings => {
            Ok(IpcResponse::ok(serde_json::json!({
                "ui": {
                    "width": cfg.ui.width,
                    "height": cfg.ui.height,
                    "anchor": cfg.ui.anchor,
                    "opacity": cfg.ui.opacity,
                    "blur": cfg.ui.blur
                },
                "grid": {
                    "thumb_size": cfg.grid.thumb_size,
                    "columns": cfg.grid.columns
                },
                "behavior": {
                    "dedupe": cfg.behavior.dedupe
                }
            })))
        }
    }
}

struct DeleteAllResult {
    deleted_items: u64,
    deleted_images: u64,
}

async fn list_items(conn: &Arc<Mutex<rusqlite::Connection>>, limit: u32, starred_only: bool) -> Result<Vec<ItemSummary>> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
        
        let sql = if starred_only {
            "SELECT id, title, body, created_at, updated_at, last_used, starred, hash,
             EXISTS (SELECT 1 FROM images WHERE images.item_id = items.id) as has_image
             FROM items WHERE starred = 1 ORDER BY last_used DESC LIMIT ?"
        } else {
            "SELECT id, title, body, created_at, updated_at, last_used, starred, hash,
             EXISTS (SELECT 1 FROM images WHERE images.item_id = items.id) as has_image
             FROM items ORDER BY starred DESC, last_used DESC LIMIT ?"
        };
        
        let mut stmt = conn.prepare(sql)?;

        let rows = stmt
            .query_map([limit], |row| {
                let id: i64 = row.get(0)?;
                let has_image: i64 = row.get(8)?;
                let hash: Option<String> = row.get(7)?;
                
                // Build thumbnail path for images
                let thumbnail_path = if has_image != 0 && hash.is_some() {
                    let thumbs_dir = crate::db::default_data_dir()
                        .map(|d| d.join("images/thumbs"))
                        .ok();
                    thumbs_dir.map(|d| d.join(format!("{}.png", hash.as_ref().unwrap())).to_string_lossy().to_string())
                } else {
                    None
                };
                
                Ok(ItemSummary {
                    id,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    last_used: row.get(5)?,
                    starred: row.get::<_, i64>(6)? != 0,
                    hash,
                    has_image: has_image != 0,
                    thumbnail_path,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    })
    .await?
}

async fn search_items(conn: &Arc<Mutex<rusqlite::Connection>>, query: &str, limit: u32) -> Result<Vec<ItemSummary>> {
    let conn = conn.clone();
    let query = build_fts_prefix_query(query);
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT items.id, items.title, items.body, items.created_at, items.updated_at, items.last_used, items.starred, items.hash,
             EXISTS (SELECT 1 FROM images WHERE images.item_id = items.id) as has_image
             FROM items_fts JOIN items ON items_fts.rowid = items.id
             WHERE items_fts MATCH ?
             ORDER BY rank
             LIMIT ?",
        )?;

        let rows = stmt
            .query_map((&query, limit), |row| {
                let id: i64 = row.get(0)?;
                let has_image: i64 = row.get(8)?;
                let hash: Option<String> = row.get(7)?;
                
                // Build thumbnail path for images
                let thumbnail_path = if has_image != 0 && hash.is_some() {
                    let thumbs_dir = crate::db::default_data_dir()
                        .map(|d| d.join("images/thumbs"))
                        .ok();
                    thumbs_dir.map(|d| d.join(format!("{}.png", hash.as_ref().unwrap())).to_string_lossy().to_string())
                } else {
                    None
                };
                
                Ok(ItemSummary {
                    id,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    last_used: row.get(5)?,
                    starred: row.get::<_, i64>(6)? != 0,
                    hash,
                    has_image: has_image != 0,
                    thumbnail_path,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    })
    .await?
}

/// Convert a user search string into an FTS5-compatible prefix query.
///
/// Example:
/// - input:  "hel wor"
/// - output: "hel* wor*"
///
/// Notes:
/// - We intentionally avoid passing through FTS operators or quotes from user input.
/// - Only ASCII letter/digit/underscore/hyphen are kept; everything else becomes a separator.
/// - Empty/too-short queries result in an empty string (caller can treat as "no search").
fn build_fts_prefix_query(input: &str) -> String {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        let keep = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
        if keep {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    // Avoid generating a massive MATCH string.
    if tokens.len() > 12 {
        tokens.truncate(12);
    }

    tokens
        .into_iter()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn gallery_items(conn: &Arc<Mutex<rusqlite::Connection>>, limit: u32) -> Result<Vec<ItemSummary>> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
        let mut stmt = conn.prepare(
            "SELECT items.id, items.title, items.body, items.created_at, items.updated_at, items.last_used, items.starred, items.hash,
             1 as has_image
             FROM items
             WHERE EXISTS (SELECT 1 FROM images WHERE images.item_id = items.id)
             ORDER BY items.last_used DESC
             LIMIT ?",
        )?;

        let rows = stmt
            .query_map([limit], |row| {
                let id: i64 = row.get(0)?;
                let hash: Option<String> = row.get(7)?;
                
                // Build thumbnail path for images (always present in gallery)
                let thumbnail_path = if hash.is_some() {
                    let thumbs_dir = crate::db::default_data_dir()
                        .map(|d| d.join("images/thumbs"))
                        .ok();
                    thumbs_dir.map(|d| d.join(format!("{}.png", hash.as_ref().unwrap())).to_string_lossy().to_string())
                } else {
                    None
                };
                
                Ok(ItemSummary {
                    id,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    last_used: row.get(5)?,
                    starred: row.get::<_, i64>(6)? != 0,
                    hash,
                    has_image: true,
                    thumbnail_path,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    })
    .await?
}

async fn star_item(conn: &Arc<Mutex<rusqlite::Connection>>, id: i64, value: bool) -> Result<u64> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;
        let updated = conn.execute(
            "UPDATE items SET starred = ? WHERE id = ?",
            rusqlite::params![if value { 1 } else { 0 }, id],
        )? as u64;
        Ok(updated)
    })
    .await?
}

async fn copy_to_clipboard(conn: &Arc<Mutex<rusqlite::Connection>>, id: i64) -> Result<()> {
    // Fetch item and possible image bytes.
    let conn = conn.clone();
    let item = tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;

        // Try image first.
        let image_row: Option<(String, Vec<u8>)> = conn
            .query_row(
                "SELECT mime, bytes FROM images WHERE item_id = ? LIMIT 1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((mime, bytes)) = image_row {
            return Ok(CopyPayload::Image { mime, bytes });
        }

        // Fallback to text.
        let text: Option<String> = conn
            .query_row(
                "SELECT body FROM items WHERE id = ?",
                [id],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(body) = text {
            return Ok(CopyPayload::Text { body });
        }

        Err(anyhow!("item not found"))
    })
    .await??;

    match item {
        CopyPayload::Image { mime, bytes } => {
            let mut child = Command::new("wl-copy")
                .arg("-t")
                .arg(&mime)
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("failed to spawn wl-copy for image")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(&bytes)
                    .await
                    .context("failed to write to wl-copy")?;
            }

            let status = child.wait().await.context("failed to wait on wl-copy")?;
            if !status.success() {
                return Err(anyhow!("wl-copy failed"));
            }
        }
        CopyPayload::Text { body } => {
            let mut child = Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("failed to spawn wl-copy for text")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(body.as_bytes())
                    .await
                    .context("failed to write to wl-copy")?;
            }

            let status = child.wait().await.context("failed to wait on wl-copy")?;
            if !status.success() {
                return Err(anyhow!("wl-copy failed"));
            }
        }
    }

    Ok(())
}

enum CopyPayload {
    Image { mime: String, bytes: Vec<u8> },
    Text { body: String },
}

async fn delete_items(conn: &Arc<Mutex<rusqlite::Connection>>, ids: Vec<i64>) -> Result<u64> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;

        let tx = conn.unchecked_transaction()?;

        // Collect hashes for items that will be deleted (for thumbnail cleanup).
        let mut hashes: Vec<String> = Vec::new();
        {
            // Use a placeholder for each ID in the IN clause.
            let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT hash FROM items WHERE id IN ({}) AND starred = 0 AND hash IS NOT NULL",
                placeholders
            );
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(
                rusqlite::params_from_iter(ids.iter()),
                |row| row.get::<_, String>(0),
            )?;
            for r in rows {
                hashes.push(r?);
            }
        }

        // Delete images for the specified items (only unstarred ones).
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql_del_imgs = format!(
            "DELETE FROM images WHERE item_id IN (SELECT id FROM items WHERE id IN ({}) AND starred = 0)",
            placeholders
        );
        tx.execute(&sql_del_imgs, rusqlite::params_from_iter(ids.iter()))?;

        // Delete the items themselves (only unstarred ones).
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql_del_items = format!(
            "DELETE FROM items WHERE id IN ({}) AND starred = 0",
            placeholders
        );
        let deleted = tx.execute(&sql_del_items, rusqlite::params_from_iter(ids.iter()))? as u64;

        tx.commit()?;

        // Best-effort file cleanup outside the transaction.
        // Stored thumbnails follow: ~/.local/share/memoria/images/thumbs/<hash>.png
        if let Ok(data_dir) = crate::db::default_data_dir() {
            let thumbs_dir = data_dir.join("images/thumbs");
            for hash in hashes {
                let p = thumbs_dir.join(format!("{hash}.png"));
                let _ = std::fs::remove_file(&p);
            }
        }

        Ok(deleted)
    })
    .await?
}

async fn delete_all_except_starred(conn: &Arc<Mutex<rusqlite::Connection>>) -> Result<DeleteAllResult> {
    let conn = conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().map_err(|e| anyhow!("lock poisoned: {e}"))?;

        let tx = conn.unchecked_transaction()?;

        // Collect hashes for items that are about to be deleted (so we can remove thumbnails).
        let mut hashes: Vec<String> = Vec::new();
        {
            let mut stmt = tx.prepare("SELECT hash FROM items WHERE starred = 0 AND hash IS NOT NULL")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for r in rows {
                hashes.push(r?);
            }
        }

        // Delete images for non-starred items first.
        let deleted_images = tx.execute(
            "DELETE FROM images WHERE item_id IN (SELECT id FROM items WHERE starred = 0)",
            [],
        )? as u64;

        // Delete the items themselves (images table has ON DELETE CASCADE too, but we already removed rows).
        let deleted_items = tx.execute("DELETE FROM items WHERE starred = 0", [])? as u64;

        tx.commit()?;

        // Best-effort file cleanup outside the transaction.
        // Stored thumbnails currently follow: ~/.local/share/memoria/images/thumbs/<hash>.png
        if let Ok(data_dir) = crate::db::default_data_dir() {
            let thumbs_dir = data_dir.join("images/thumbs");
            for hash in hashes {
                let p = thumbs_dir.join(format!("{hash}.png"));
                let _ = std::fs::remove_file(&p);
            }
        }

        Ok(DeleteAllResult {
            deleted_items,
            deleted_images,
        })
    })
    .await?
}
