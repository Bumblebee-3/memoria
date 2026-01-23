use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
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

pub fn default_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    Ok(home.join(".config/memoria/config.toml"))
}

pub fn load_or_default(path: &Path) -> Result<Config> {
    if !path.exists() {
        warn!("config file not found, creating with defaults: {}", path.display());
        
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
        }

        let default_cfg = Config::default();
        let toml_string = toml::to_string_pretty(&default_cfg)
            .context("failed to serialize default config")?;
        
        std::fs::write(path, toml_string)
            .with_context(|| format!("failed to write default config: {}", path.display()))?;
        
        info!("created default config at: {}", path.display());
        return Ok(default_cfg);
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;

    match toml::from_str::<Config>(&raw) {
        Ok(cfg) => {
            info!("loaded config from: {}", path.display());
            Ok(cfg)
        }
        Err(err) => {
            if raw.trim().is_empty() {
                warn!("config file is empty, using defaults");
                Ok(Config::default())
            } else {
                match toml::from_str::<toml::Value>(&raw) {
                    Ok(_) => {
                        warn!("config file has missing or invalid fields, using defaults where needed");
                        warn!("parse warning: {}", err);
                        Ok(Config::default())
                    }
                    Err(_) => {
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

#[allow(dead_code)]
pub fn load_from_file(path: &Path) -> Result<Config> {
    load_or_default(path)
}
