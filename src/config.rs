use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::atomic_file;
use crate::paths;
use crate::release_manifest::{ManifestFrameRate, ManifestLaunchOptions};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LauncherConfig {
    pub install_dir: Option<PathBuf>,
    pub channel: ReleaseChannel,
    #[serde(default = "default_download_cache_limit")]
    pub download_cache_limit: usize,
    pub pre_launch_command: String,
    #[serde(default)]
    pub launch_arguments_mode: LaunchArgumentsMode,
    #[serde(default)]
    pub frame_rate: FrameRatePreference,
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
            frame_rate: FrameRatePreference::default(),
            game_args: Vec::new(),
        }
    }
}

pub fn default_download_cache_limit() -> usize {
    3
}

impl LauncherConfig {
    pub fn effective_install_dir(&self) -> PathBuf {
        self.install_dir
            .clone()
            .unwrap_or_else(paths::default_install_dir)
    }

    pub fn ensure_install_dir(&mut self) -> PathBuf {
        let install_dir = self.effective_install_dir();
        self.install_dir = Some(install_dir.clone());
        install_dir
    }

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
        atomic_file::write(&path, contents.as_bytes())
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameRateMode {
    #[default]
    Auto,
    Preset,
    Custom,
}

impl FrameRateMode {
    pub fn from_ui_index(index: i32) -> Self {
        match index {
            1 => Self::Preset,
            2 => Self::Custom,
            _ => Self::Auto,
        }
    }

    pub fn ui_index(self) -> i32 {
        match self {
            Self::Auto => 0,
            Self::Preset => 1,
            Self::Custom => 2,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrameRatePreference {
    #[serde(default)]
    pub mode: FrameRateMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<u32>,
}

impl FrameRatePreference {
    pub fn resolved_argument(&self, definition: &ManifestFrameRate) -> String {
        match self.mode {
            FrameRateMode::Auto => "auto".to_string(),
            FrameRateMode::Preset | FrameRateMode::Custom => self
                .value
                .filter(|value| (definition.custom_min..=definition.custom_max).contains(value))
                .map(|value| value.to_string())
                .unwrap_or_else(|| "auto".to_string()),
        }
    }
}

impl LauncherConfig {
    pub fn effective_game_args(
        &self,
        launch_options: Option<&ManifestLaunchOptions>,
    ) -> Vec<String> {
        let mut args = match self.launch_arguments_mode {
            LaunchArgumentsMode::GameDefaults => self.game_args.clone(),
            LaunchArgumentsMode::Recommended => {
                let mut args = launch_options
                    .map(recommended_launch_option_args)
                    .unwrap_or_default();
                args.extend(self.game_args.clone());
                args
            }
            LaunchArgumentsMode::Custom => self.game_args.clone(),
        };

        if let Some(frame_rate) = launch_options.and_then(|options| options.frame_rate.as_ref()) {
            remove_controlled_argument(&mut args, &frame_rate.flag);
            args.insert(
                0,
                format!(
                    "{}={}",
                    frame_rate.flag,
                    self.frame_rate.resolved_argument(frame_rate)
                ),
            );
        }

        args
    }
}

fn recommended_launch_option_args(launch_options: &ManifestLaunchOptions) -> Vec<String> {
    launch_options
        .game_arguments
        .iter()
        .flat_map(|argument| match argument.recommended {
            Some(recommended) if recommended != argument.default => {
                vec![argument.flag.clone(), recommended.to_string()]
            }
            _ => Vec::new(),
        })
        .collect()
}

fn remove_controlled_argument(args: &mut Vec<String>, flag: &str) {
    let assignment_prefix = format!("{flag}=");
    let mut index = 0;
    while index < args.len() {
        if args[index].starts_with(&assignment_prefix) {
            args.remove(index);
        } else if args[index] == flag {
            args.remove(index);
            if index < args.len() && !args[index].starts_with("--") {
                args.remove(index);
            }
        } else {
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release_manifest::{ManifestAutoFrameRate, ManifestFrameRate, ManifestGameArgument};

    fn launch_options() -> ManifestLaunchOptions {
        ManifestLaunchOptions {
            frame_rate: Some(ManifestFrameRate {
                flag: "--fps".to_string(),
                auto: ManifestAutoFrameRate {
                    fallback: 120,
                    step: 24,
                    maximum: 240,
                },
                custom_min: 1,
                custom_max: 10_000,
            }),
            game_arguments: vec![ManifestGameArgument {
                name: "want-zoom".to_string(),
                flag: "--want-zoom".to_string(),
                default: false,
                recommended: Some(true),
                config_key: None,
            }],
        }
    }

    #[test]
    fn effective_install_dir_uses_default_when_unset() {
        let config = LauncherConfig::default();

        assert_eq!(config.effective_install_dir(), paths::default_install_dir());
    }

    #[test]
    fn ensure_install_dir_persists_default_path() {
        let mut config = LauncherConfig::default();

        let install_dir = config.ensure_install_dir();

        assert_eq!(config.install_dir.as_deref(), Some(install_dir.as_path()));
        assert_eq!(install_dir, paths::default_install_dir());
    }

    #[test]
    fn defaults_to_recommended_launch_arguments_mode() {
        let config = LauncherConfig::default();

        assert_eq!(
            config.launch_arguments_mode,
            LaunchArgumentsMode::Recommended
        );
        assert_eq!(config.frame_rate, FrameRatePreference::default());
        assert!(config.effective_game_args(None).is_empty());
    }

    #[test]
    fn custom_launch_arguments_use_saved_arguments() {
        let config = LauncherConfig {
            launch_arguments_mode: LaunchArgumentsMode::Custom,
            game_args: vec!["--want-zoom".to_string(), "true".to_string()],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args(Some(&launch_options())),
            vec![
                "--fps=auto".to_string(),
                "--want-zoom".to_string(),
                "true".to_string()
            ]
        );
    }

    #[test]
    fn recommended_launch_arguments_can_come_from_release_metadata() {
        let config = LauncherConfig::default();

        assert_eq!(
            config.effective_game_args(Some(&launch_options())),
            vec![
                "--fps=auto".to_string(),
                "--want-zoom".to_string(),
                "true".to_string()
            ]
        );
    }

    #[test]
    fn non_custom_modes_append_extra_arguments() {
        let config = LauncherConfig {
            game_args: vec!["--debug".to_string()],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args(Some(&launch_options())),
            vec![
                "--fps=auto".to_string(),
                "--want-zoom".to_string(),
                "true".to_string(),
                "--debug".to_string()
            ]
        );
    }

    #[test]
    fn game_defaults_keep_extra_arguments_only() {
        let config = LauncherConfig {
            launch_arguments_mode: LaunchArgumentsMode::GameDefaults,
            game_args: vec!["--debug".to_string()],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args(Some(&launch_options())),
            vec!["--fps=auto".to_string(), "--debug".to_string()]
        );
    }

    #[test]
    fn explicit_frame_rate_is_preserved_and_owned_by_the_control() {
        let config = LauncherConfig {
            frame_rate: FrameRatePreference {
                mode: FrameRateMode::Custom,
                value: Some(300),
            },
            game_args: vec![
                "--fps".to_string(),
                "60".to_string(),
                "--debug".to_string(),
                "--fps=144".to_string(),
            ],
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args(Some(&launch_options())),
            vec![
                "--fps=300".to_string(),
                "--want-zoom".to_string(),
                "true".to_string(),
                "--debug".to_string()
            ]
        );
    }

    #[test]
    fn invalid_saved_frame_rate_falls_back_to_auto() {
        let config = LauncherConfig {
            frame_rate: FrameRatePreference {
                mode: FrameRateMode::Custom,
                value: Some(20_000),
            },
            ..LauncherConfig::default()
        };

        assert_eq!(
            config.effective_game_args(Some(&launch_options()))[0],
            "--fps=auto"
        );
    }

    #[test]
    fn auto_and_explicit_frame_rates_remain_distinct() {
        let options = launch_options();
        let automatic = LauncherConfig::default();
        let explicit = LauncherConfig {
            frame_rate: FrameRatePreference {
                mode: FrameRateMode::Preset,
                value: Some(120),
            },
            ..LauncherConfig::default()
        };

        assert_eq!(
            automatic.effective_game_args(Some(&options))[0],
            "--fps=auto"
        );
        assert_eq!(explicit.effective_game_args(Some(&options))[0], "--fps=120");
    }

    #[test]
    fn frame_rate_is_not_passed_to_versions_without_metadata() {
        let config = LauncherConfig {
            frame_rate: FrameRatePreference {
                mode: FrameRateMode::Custom,
                value: Some(300),
            },
            ..LauncherConfig::default()
        };

        assert!(config.effective_game_args(None).is_empty());
    }

    #[test]
    fn existing_config_without_frame_rate_uses_default() {
        let config: LauncherConfig = serde_json::from_str(
            r#"{
                "install_dir": null,
                "channel": "stable",
                "download_cache_limit": 3,
                "pre_launch_command": "",
                "launch_arguments_mode": "recommended",
                "game_args": []
            }"#,
        )
        .unwrap();

        assert_eq!(config.frame_rate, FrameRatePreference::default());
    }
}
