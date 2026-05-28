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
    #[serde(default)]
    pub launch_arguments_mode: LaunchArgumentsMode,
    pub game_args: Vec<String>,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        Self {
            install_dir: None,
            channel: ReleaseChannel::Stable,
            pre_launch_command: String::new(),
            launch_arguments_mode: LaunchArgumentsMode::Recommended,
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchArgumentsMode {
    GameDefaults,
    #[default]
    Recommended,
    Custom,
}

impl LaunchArgumentsMode {
    pub fn from_ui_index(index: i32) -> Self {
        match index {
            0 => Self::GameDefaults,
            2 => Self::Custom,
            _ => Self::Recommended,
        }
    }

    pub fn ui_index(self) -> i32 {
        match self {
            Self::GameDefaults => 0,
            Self::Recommended => 1,
            Self::Custom => 2,
        }
    }
}

impl LauncherConfig {
    pub fn effective_game_args(&self) -> Vec<String> {
        match self.launch_arguments_mode {
            LaunchArgumentsMode::GameDefaults => Vec::new(),
            LaunchArgumentsMode::Recommended => recommended_game_args(),
            LaunchArgumentsMode::Custom => self.game_args.clone(),
        }
    }
}

fn recommended_game_args() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_recommended_launch_arguments_mode() {
        let config = LauncherConfig::default();

        assert_eq!(
            config.launch_arguments_mode,
            LaunchArgumentsMode::Recommended
        );
        assert!(config.effective_game_args().is_empty());
    }

    #[test]
    fn custom_launch_arguments_use_saved_arguments() {
        let config = LauncherConfig {
            launch_arguments_mode: LaunchArgumentsMode::Custom,
            game_args: vec!["--want-zoom".to_string(), "true".to_string()],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args(),
            vec!["--want-zoom".to_string(), "true".to_string()]
        );
    }
}
