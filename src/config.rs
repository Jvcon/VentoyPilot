use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Microsoft software-download connector session parameters.
/// These are the (currently) fixed values used by Fido.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MsConfig {
    pub org_id: String,
    pub profile_id: String,
    pub instance_id: String,
}

impl Default for MsConfig {
    fn default() -> Self {
        Self {
            org_id: "y6jn8c31".to_string(),
            profile_id: "606624d44113".to_string(),
            instance_id: "560dc9f3-1aa5-4a2f-b63c-9e18f8d0e175".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Where downloaded ISOs are stored.
    pub download_dir: PathBuf,
    /// Default language, e.g. "zh-CN" or "Chinese (Simplified)".
    pub default_lang: String,
    /// Default architecture: x64 | arm64 | x86.
    pub default_arch: String,
    /// Default release, e.g. "25H2" or "latest".
    pub default_release: String,
    /// Default edition (Fido -Ed), e.g. "desktop" / "Pro".
    pub default_edition: Option<String>,
    /// Microsoft connector session parameters.
    pub ms: MsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            download_dir: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")),
            default_lang: "en-US".to_string(),
            default_arch: "x64".to_string(),
            default_release: "latest".to_string(),
            default_edition: None,
            ms: MsConfig::default(),
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ventoypilot")
            .join("config.toml")
    }

    /// Load the config file, falling back to defaults when it does not exist.
    pub fn load() -> Result<Config> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| Error::Config(format!("cannot read {}: {e}", path.display())))?;
        let cfg: Config = toml::from_str(&raw)
            .map_err(|e| Error::Config(format!("invalid {}: {e}", path.display())))?;
        Ok(cfg)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("serialize config: {e}")))?;
        std::fs::write(&path, raw)?;
        Ok(())
    }

    /// Resolve the effective download directory (config or CLI override).
    pub fn download_dir(&self, cli_override: Option<&Path>) -> PathBuf {
        let dir = match cli_override {
            Some(p) => p.to_path_buf(),
            None => self.download_dir.clone(),
        };
        expand_path(&dir)
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "download_dir" => self.download_dir = PathBuf::from(value),
            "lang" => self.default_lang = value.to_string(),
            "arch" => self.default_arch = value.to_string(),
            "release" => self.default_release = value.to_string(),
            "edition" => self.default_edition = Some(value.to_string()),
            "ms.org_id" => self.ms.org_id = value.to_string(),
            "ms.profile_id" => self.ms.profile_id = value.to_string(),
            "ms.instance_id" => self.ms.instance_id = value.to_string(),
            _ => {
                return Err(Error::Config(format!(
                    "unknown config key '{key}' (valid: download_dir, lang, arch, release, edition, ms.org_id, ms.profile_id, ms.instance_id)"
                )))
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "download_dir" => Some(self.download_dir.display().to_string()),
            "lang" => Some(self.default_lang.clone()),
            "arch" => Some(self.default_arch.clone()),
            "release" => Some(self.default_release.clone()),
            "edition" => self.default_edition.clone(),
            "ms.org_id" => Some(self.ms.org_id.clone()),
            "ms.profile_id" => Some(self.ms.profile_id.clone()),
            "ms.instance_id" => Some(self.ms.instance_id.clone()),
            _ => None,
        }
    }

    /// All known keys, for `config list`.
    pub const KEYS: &'static [&'static str] = &[
        "download_dir",
        "lang",
        "arch",
        "release",
        "edition",
        "ms.org_id",
        "ms.profile_id",
        "ms.instance_id",
    ];
}

/// Expand a leading `~` into the user's home directory.
pub fn expand_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if s == "~" {
        return dirs::home_dir().unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    path.to_path_buf()
}
