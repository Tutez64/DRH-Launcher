use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::paths;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LauncherConfig {
    pub install_dir: Option<PathBuf>,
    pub channel: ReleaseChannel,
    #[serde(default = "default_download_cache_limit")]
    pub download_cache_limit: usize,
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
            download_cache_limit: default_download_cache_limit(),
            pre_launch_command: String::new(),
            launch_arguments_mode: LaunchArgumentsMode::Recommended,
            game_args: Vec::new(),
        }
    }
}

pub fn default_download_cache_limit() -> usize {
    3
}

impl LauncherConfig {
    pub fn load_with_diagnostics() -> (Self, Option<String>) {
        let path = paths::config_file();
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("Could not read {}: {error}", path.display())),
                );
            }
        };

        match serde_json::from_str(&contents) {
            Ok(config) => (config, None),
            Err(error) => (
                Self::default(),
                Some(format!(
                    "Could not parse {}; using default settings: {error}",
                    path.display()
                )),
            ),
        }
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
    pub fn effective_game_args_with_recommended(&self, recommended: &[String]) -> Vec<String> {
        match self.launch_arguments_mode {
            LaunchArgumentsMode::GameDefaults => self.game_args.clone(),
            LaunchArgumentsMode::Recommended => {
                let mut args = recommended.to_vec();
                args.extend(self.game_args.clone());
                args
            }
            LaunchArgumentsMode::Custom => self.game_args.clone(),
        }
    }
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
        assert!(config.effective_game_args_with_recommended(&[]).is_empty());
    }

    #[test]
    fn custom_launch_arguments_use_saved_arguments() {
        let config = LauncherConfig {
            launch_arguments_mode: LaunchArgumentsMode::Custom,
            game_args: vec!["--want-zoom".to_string(), "true".to_string()],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args_with_recommended(&[]),
            vec!["--want-zoom".to_string(), "true".to_string()]
        );
    }

    #[test]
    fn recommended_launch_arguments_can_come_from_release_metadata() {
        let config = LauncherConfig::default();

        assert_eq!(
            config.effective_game_args_with_recommended(&["--want-zoom".to_string()]),
            vec!["--want-zoom".to_string()]
        );
    }

    #[test]
    fn non_custom_modes_append_saved_extra_arguments() {
        let config = LauncherConfig {
            game_args: vec!["--debug".to_string()],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args_with_recommended(&["--want-zoom".to_string()]),
            vec!["--want-zoom".to_string(), "--debug".to_string()]
        );
    }
}
