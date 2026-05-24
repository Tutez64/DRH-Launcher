use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::paths;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LauncherConfig {
    pub install_dir: Option<PathBuf>,
    pub channel: ReleaseChannel,
    pub pre_launch_command: String,
    pub game_args: Vec<String>,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            install_dir: None,
            channel: ReleaseChannel::Stable,
            pre_launch_command: String::new(),
            game_args: Vec::new(),
        }
    }
}

impl LauncherConfig {
    pub fn load() -> Self {
        let path = paths::config_file();
        let Ok(contents) = fs::read_to_string(path) else {
            return Self::default();
        };

        serde_json::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = paths::config_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::write(path, contents)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseChannel {
    Stable,
    Beta,
}
