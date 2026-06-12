use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to `$HOME/.config`.
fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".config"))
}

/// XDG data dir, honoring `$XDG_DATA_HOME` and falling back to `$HOME/.local/share`.
///
/// We deliberately do NOT use the `dirs` config/data helpers: those honor
/// `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` only on Linux. On macOS they resolve via system
/// APIs and return `~/Library/...`, ignoring the env vars. These helpers resolve to the
/// same XDG layout on every platform.
pub fn xdg_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".local").join("share"))
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home;
    }
    PathBuf::from(path)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DuplicateAction {
    #[default]
    Skip,
    Dedup,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// Whether to run dashify on files before moving them
    pub dashify: bool,

    /// Source directories to scan for files
    pub sources: Vec<String>,

    /// Rules mapping destination directory -> list of extensions
    pub rules: HashMap<String, Vec<String>>,

    /// What to do when destination file already exists with identical content
    #[serde(rename = "on-duplicate")]
    pub on_duplicate: DuplicateAction,

    /// Glob patterns for filenames to exclude from processing
    #[serde(default)]
    pub exclude: Vec<String>,

    /// Preserve subdirectory structure when moving files
    #[serde(rename = "preserve-paths", default)]
    pub preserve_paths: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dashify: true,
            sources: vec!["~/Downloads".to_string()],
            rules: HashMap::new(),
            on_duplicate: DuplicateAction::default(),
            exclude: Vec::new(),
            preserve_paths: false,
        }
    }
}

impl Config {
    /// Get expanded source paths
    pub fn source_paths(&self) -> Vec<PathBuf> {
        self.sources.iter().map(|s| expand_tilde(s)).collect()
    }

    /// Build a reverse lookup: extension -> destination path
    pub fn extension_map(&self) -> HashMap<String, PathBuf> {
        let mut map = HashMap::new();
        for (dest, exts) in &self.rules {
            let dest_path = expand_tilde(dest);
            for ext in exts {
                // Normalize: store without leading dot, lowercase
                let normalized = ext.trim_start_matches('.').to_lowercase();
                map.insert(normalized, dest_path.clone());
            }
        }
        map
    }

    /// Load configuration with fallback chain
    pub fn load(config_path: Option<&PathBuf>) -> Result<Self> {
        if let Some(path) = config_path {
            return Self::load_from_file(path).context(format!("Failed to load config from {}", path.display()));
        }

        // Try ~/.config/kondo/kondo.yml
        if let Some(config_dir) = xdg_config_dir() {
            let project_name = env!("CARGO_PKG_NAME");
            let primary_config = config_dir.join(project_name).join(format!("{}.yml", project_name));
            if primary_config.exists() {
                match Self::load_from_file(&primary_config) {
                    Ok(config) => return Ok(config),
                    Err(e) => {
                        log::warn!("Failed to load config from {}: {}", primary_config.display(), e);
                    }
                }
            }
        }

        // Try ./kondo.yml
        let project_name = env!("CARGO_PKG_NAME");
        let fallback_config = PathBuf::from(format!("{}.yml", project_name));
        if fallback_config.exists() {
            match Self::load_from_file(&fallback_config) {
                Ok(config) => return Ok(config),
                Err(e) => {
                    log::warn!("Failed to load config from {}: {}", fallback_config.display(), e);
                }
            }
        }

        log::info!("No config file found, using defaults");
        Ok(Self::default())
    }

    fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(&path).context("Failed to read config file")?;
        let config: Self = serde_yaml::from_str(&content).context("Failed to parse config file")?;
        log::info!("Loaded config from: {}", path.as_ref().display());
        Ok(config)
    }
}
