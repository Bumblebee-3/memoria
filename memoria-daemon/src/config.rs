use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Top-level configuration.
///
/// This matches the TOML structure requested:
///
/// - `retention.days`
/// - `ui.width`, `ui.height`, `ui.anchor`, `ui.opacity`, `ui.blur`
/// - `grid.thumb_size`, `grid.columns`
/// - `behavior.dedupe`
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub retention: Retention,
    pub ui: Ui,
    pub grid: Grid,
    pub behavior: Behavior,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            retention: Retention::default(),
            ui: Ui::default(),
            grid: Grid::default(),
            behavior: Behavior::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Retention {
    pub days: u32,
    pub delete_unstarred_only: bool,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            days: 30,
            delete_unstarred_only: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Ui {
    pub width: u32,
    pub height: u32,
    pub anchor: String,
    pub opacity: f32,
    pub blur: f32,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            width: 480,
            height: 640,
            anchor: "top-right".to_string(),
            opacity: 0.92,
            blur: 12.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Grid {
    pub thumb_size: u32,
    pub columns: u32,
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            thumb_size: 104,
            columns: 3,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Behavior {
    pub dedupe: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Self { dedupe: true }
    }
}

/// Resolve the user config path: `~/.config/memoria/config.toml`.
pub fn default_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    Ok(home.join(".config/memoria/config.toml"))
}

/// Load configuration from disk, or create with defaults if missing.
///
/// Behavior:
/// - If config file doesn't exist: create it with defaults
/// - If config file exists but has missing fields: merge with defaults and warn
/// - If config file is completely invalid TOML: return error
/// - If config directory can't be created: return error
///
/// This ensures the daemon can always start with a valid configuration.
pub fn load_or_default(path: &Path) -> Result<Config> {
    if !path.exists() {
        warn!("config file not found, creating with defaults: {}", path.display());
        
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
        }

        // Create default config file
        let default_cfg = Config::default();
        let toml_string = toml::to_string_pretty(&default_cfg)
            .context("failed to serialize default config")?;
        
        std::fs::write(path, toml_string)
            .with_context(|| format!("failed to write default config: {}", path.display()))?;
        
        info!("created default config at: {}", path.display());
        return Ok(default_cfg);
    }

    // File exists, try to load it
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    // Try to parse as TOML
    match toml::from_str::<Config>(&raw) {
        Ok(cfg) => {
            info!("loaded config from: {}", path.display());
            Ok(cfg)
        }
        Err(err) => {
            // Check if it's a syntax error or just missing fields
            if raw.trim().is_empty() {
                warn!("config file is empty, using defaults");
                Ok(Config::default())
            } else {
                // Try to parse as a generic value to see if TOML is valid
                match toml::from_str::<toml::Value>(&raw) {
                    Ok(_) => {
                        // TOML is valid but doesn't match our structure
                        // This means serde(default) should handle it
                        warn!("config file has missing or invalid fields, using defaults where needed");
                        warn!("parse warning: {}", err);
                        
                        // Return defaults with a warning - serde should have handled this
                        // but if we get here, fall back to full defaults
                        Ok(Config::default())
                    }
                    Err(_) => {
                        // Invalid TOML syntax - this is a hard error
                        Err(anyhow::anyhow!(
                            "INVALID CONFIG.TOML: syntax error: {}\nPath: {}",
                            err,
                            path.display()
                        ))
                    }
                }
            }
        }
    }
}

/// Legacy function kept for compatibility - now just calls load_or_default.
#[allow(dead_code)]
pub fn load_from_file(path: &Path) -> Result<Config> {
    load_or_default(path)
}
