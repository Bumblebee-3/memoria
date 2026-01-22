use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// Resolve the data directory: `~/.local/share/memoria/`.
pub fn default_data_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    Ok(home.join(".local/share/memoria"))
}

/// Resolve the database path: `~/.local/share/memoria/memoria.db`.
pub fn default_db_path() -> Result<PathBuf> {
    Ok(default_data_dir()?.join("memoria.db"))
}

/// Ensures the data directory exists.
pub fn ensure_data_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create data dir: {}", dir.display()))?;
    Ok(())
}

/// Open the SQLite database (creating it if missing) and run schema migrations.
///
/// Uses `rusqlite` and enables basic pragmas. The schema includes:
/// - `items` table
/// - `images` table
/// - `items_fts` FTS5 virtual table for text search
pub fn open_and_init(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("failed to open db: {}", db_path.display()))?;

    // Pragmas are applied per-connection.
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("failed to enable foreign_keys")?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .context("failed to enable WAL")?;

    // Schema: kept intentionally minimal but extensible.
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS items (
            id            INTEGER PRIMARY KEY,
            created_at    INTEGER NOT NULL,
            updated_at    INTEGER NOT NULL,
            last_used     INTEGER,
            starred       INTEGER DEFAULT 0,
            title         TEXT,
            body          TEXT,
            hash          TEXT,
            UNIQUE(hash)
        );

        CREATE TABLE IF NOT EXISTS images (
            id            INTEGER PRIMARY KEY,
            item_id       INTEGER NOT NULL,
            created_at    INTEGER NOT NULL,
            mime          TEXT,
            bytes         BLOB,
            FOREIGN KEY(item_id) REFERENCES items(id) ON DELETE CASCADE
        );

        -- Full-text search virtual table.
        -- NOTE: Requires SQLite built with FTS5 (enabled via rusqlite `bundled`).
        CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
            title,
            body,
            content='items',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS items_ai AFTER INSERT ON items BEGIN
            INSERT INTO items_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
        END;

        CREATE TRIGGER IF NOT EXISTS items_ad AFTER DELETE ON items BEGIN
            INSERT INTO items_fts(items_fts, rowid, title, body) VALUES('delete', old.id, old.title, old.body);
        END;

        CREATE TRIGGER IF NOT EXISTS items_au AFTER UPDATE ON items BEGIN
            INSERT INTO items_fts(items_fts, rowid, title, body) VALUES('delete', old.id, old.title, old.body);
            INSERT INTO items_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
        END;
        "#,
    )
    .context("failed to initialize schema")?;

    // A tiny no-op sanity query to ensure the connection is usable.
    let _: i64 = conn.query_row("SELECT 1", params![], |row| row.get(0))?;

    Ok(conn)
}
