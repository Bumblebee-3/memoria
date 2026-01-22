use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Top-level configuration.
///
/// This matches the TOML structure requested:
///
/// - `retention.days`
/// - `ui.width`, `ui.height`, `ui.anchor`, `ui.opacity`, `ui.blur`
/// - `grid.thumb_size`, `grid.columns`
/// - `behavior.dedupe`
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub retention: Retention,
    pub ui: Ui,
    pub grid: Grid,
    pub behavior: Behavior,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Retention {
    pub days: u32,
    #[serde(default)]
    pub delete_unstarred_only: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ui {
    pub width: u32,
    pub height: u32,
    pub anchor: String,
    pub opacity: f32,
    pub blur: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Grid {
    pub thumb_size: u32,
    pub columns: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Behavior {
    pub dedupe: bool,
}

/// Resolve the user config path: `~/.config/memoria/config.toml`.
pub fn default_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    Ok(home.join(".config/memoria/config.toml"))
}

/// Load configuration from disk.
///
/// We require the file to exist and contain all requested fields.
pub fn load_from_file(path: &Path) -> Result<Config> {
    if !path.exists() {
        bail!("config file does not exist: {}", path.display());
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config: {}", path.display()))?;

    let cfg: Config = toml::from_str(&raw)
        .with_context(|| format!("failed to parse config TOML: {}", path.display()))?;

    Ok(cfg)
}
