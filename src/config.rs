use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const DEFAULT_SYNC_DIRS: &[&str] = &["projects", "todos", "plans"];
const DEFAULT_PULL_INTERVAL_SECS: u64 = 300; // 5 minutes

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub remote_url: String,
    pub machine_id: String,
    pub sync_dirs: Vec<String>,
    pub pull_interval_secs: u64,
    #[serde(default = "default_local_repo_path")]
    pub local_repo_path: PathBuf,
}

fn default_local_repo_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("synclaude/repo")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            remote_url: String::new(),
            machine_id: read_machine_id().unwrap_or_else(|_| "unknown".into()),
            sync_dirs: DEFAULT_SYNC_DIRS.iter().map(|s| (*s).to_string()).collect(),
            pull_interval_secs: DEFAULT_PULL_INTERVAL_SECS,
            local_repo_path: default_local_repo_path(),
        }
    }
}

impl Config {
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine config directory")?
            .join("synclaude");
        Ok(config_dir.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            anyhow::bail!(
                "No config found at {}. Run `synclaude init <repo-url>` first.",
                path.display()
            );
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config at {}", path.display()))?;
        let config: Config =
            toml::from_str(&content).context("Failed to parse config.toml")?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn claude_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("Could not determine home directory")?;
        Ok(home.join(".claude"))
    }

    /// Returns absolute paths to the directories that should be synced.
    pub fn sync_source_paths(&self) -> Result<Vec<PathBuf>> {
        let claude_dir = Self::claude_dir()?;
        Ok(self
            .sync_dirs
            .iter()
            .map(|d| claude_dir.join(d))
            .collect())
    }

    pub fn branch_name(&self) -> String {
        format!("machine/{}", self.machine_id)
    }
}

/// Read /etc/machine-id (standard on NixOS/systemd systems).
pub fn read_machine_id() -> Result<String> {
    let id = std::fs::read_to_string("/etc/machine-id")
        .context("Failed to read /etc/machine-id")?
        .trim()
        .to_string();
    Ok(id)
}
