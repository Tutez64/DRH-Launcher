use crate::config::{FrameRateMode, FrameRatePreference, LaunchArgumentsMode, LauncherConfig};
use crate::install_state::InstallState;
use crate::release_manifest::{ManifestFrameRate, ManifestLaunchOptions};
use crate::{AppWindow, LaunchOptionView, game_install, game_launch, install_metadata};
use slint::{Model, ModelRc, VecModel};

const NO_INSTALLED_LAUNCH_OPTIONS_TEXT: &str =
    "Install DRH from the Home screen first to load and edit its launch options.";
const NO_KNOWN_LAUNCH_OPTIONS_TEXT: &str =
    "No known launch options loaded from the release manifest.";

pub(crate) fn refresh_launch_options_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    launch_options: Option<&ManifestLaunchOptions>,
    save_text: &str,
) {
    let (view_options, custom_game_args) = launch_options_view_state(config, launch_options);
    let launch_options_state = launch_options_state(&view_options);
    let frame_rate = frame_rate_view_state(config, launch_options);

    ui.set_saved_pre_launch_command(config.pre_launch_command.clone().into());
    ui.set_saved_frame_rate_mode(frame_rate.mode.ui_index());
    ui.set_saved_game_frame_rate(frame_rate.value.to_string().into());
    ui.set_saved_launch_arguments_mode(config.launch_arguments_mode.ui_index());
    ui.set_saved_custom_game_args(custom_game_args.clone().into());
    ui.set_saved_launch_options_state(launch_options_state.clone().into());

    ui.set_pre_launch_command(config.pre_launch_command.clone().into());
    apply_frame_rate_to_view(ui, &frame_rate);
    ui.set_game_frame_rate_error("".into());
    ui.set_launch_arguments_mode(config.launch_arguments_mode.ui_index());
    ui.set_custom_game_args(custom_game_args.into());
    ui.set_launch_options_save_text(save_text.into());
    ui.set_launch_argument_controls_visible(frame_rate.controls_visible);
    ui.set_launch_argument_modes_enabled(has_known_launch_arguments(launch_options));
    ui.set_launch_options_empty_text(empty_launch_options_text(config, launch_options).into());
    apply_launch_options_to_view(ui, view_options, ui.get_custom_game_args().to_string());
}

fn has_known_launch_arguments(launch_options: Option<&ManifestLaunchOptions>) -> bool {
    launch_options.is_some_and(|options| !options.game_arguments.is_empty())
}

struct FrameRateViewState {
    controls_visible: bool,
    supported: bool,
    mode: FrameRateMode,
    value: u32,
    preset_index: f32,
    presets: Vec<u32>,
    notice: String,
}

pub(crate) fn load_installed_launch_options(
    config: &LauncherConfig,
) -> Option<ManifestLaunchOptions> {
    let state = install_metadata::InstalledState::load(&config.effective_install_dir()).ok()?;
    state.active.launch_options
}

fn frame_rate_view_state(
    config: &LauncherConfig,
    launch_options: Option<&ManifestLaunchOptions>,
) -> FrameRateViewState {
    let controls_visible = game_install::inspect_install(Some(&config.effective_install_dir()))
        .state
        != InstallState::NotInstalled;
    if !controls_visible {
        return FrameRateViewState {
            controls_visible: false,
            supported: false,
            mode: FrameRateMode::Auto,
            value: 0,
            preset_index: 0.5,
            presets: Vec::new(),
            notice: "Install DRH from the Home screen first to configure its frame rate."
                .to_string(),
        };
    }
    let Some(definition) = launch_options.and_then(|options| options.frame_rate.as_ref()) else {
        return FrameRateViewState {
            controls_visible: true,
            supported: false,
            mode: FrameRateMode::Auto,
            value: 0,
            preset_index: 0.5,
            presets: Vec::new(),
            notice: "This DRH version does not support configurable frame rates. It will use its built-in frame rate.".to_string(),
        };
    };

    let presets = definition.preset_values();
    let saved_value = config.frame_rate.value.unwrap_or(definition.auto.fallback);
    let (mode, value) = match config.frame_rate.mode {
        FrameRateMode::Auto => (FrameRateMode::Auto, definition.auto.fallback),
        FrameRateMode::Preset if presets.contains(&saved_value) => {
            (FrameRateMode::Preset, saved_value)
        }
        FrameRateMode::Preset | FrameRateMode::Custom
            if (definition.custom_min..=definition.custom_max).contains(&saved_value) =>
        {
            (FrameRateMode::Custom, saved_value)
        }
        FrameRateMode::Preset | FrameRateMode::Custom => {
            (FrameRateMode::Auto, definition.auto.fallback)
        }
    };
    let preset_index = presets
        .iter()
        .position(|preset| *preset == value)
        .or_else(|| {
            presets
                .iter()
                .position(|preset| *preset == definition.auto.fallback)
        })
        .unwrap_or(0) as f32;

    FrameRateViewState {
        controls_visible,
        supported: true,
        mode,
        value,
        preset_index,
        presets,
        notice: frame_rate_notice(mode, definition),
    }
}

fn frame_rate_notice(mode: FrameRateMode, definition: &ManifestFrameRate) -> String {
    match mode {
        FrameRateMode::Auto => "DRH will choose a frame rate at startup based on the primary display. The selected value is therefore not shown in the Launcher.".to_string(),
        FrameRateMode::Preset => format!(
            "Choose a fixed frame rate in {} FPS increments. The best value depends on your display and system performance.",
            definition.auto.step
        ),
        FrameRateMode::Custom => format!(
            "Custom values can reduce performance or affect gameplay. Accepted range: {} to {} FPS; standard presets: {} to {} FPS.",
            definition.custom_min,
            definition.custom_max,
            definition.auto.step,
            definition.auto.maximum
        ),
    }
}

fn apply_frame_rate_to_view(ui: &AppWindow, state: &FrameRateViewState) {
    ui.set_frame_rate_controls_visible(state.controls_visible);
    ui.set_frame_rate_supported(state.supported);
    ui.set_frame_rate_mode(state.mode.ui_index());
    ui.set_game_frame_rate(state.value.to_string().into());
    ui.set_frame_rate_preset_index(state.preset_index);
    ui.set_frame_rate_preset_maximum(state.presets.len().saturating_sub(1).max(1) as f32);
    ui.set_frame_rate_presets(ModelRc::new(VecModel::from(
        state
            .presets
            .iter()
            .map(|value| *value as i32)
            .collect::<Vec<_>>(),
    )));
    ui.set_frame_rate_notice(state.notice.clone().into());
}

pub(crate) fn apply_frame_rate_mode_to_view(
    ui: &AppWindow,
    launch_options: Option<&ManifestLaunchOptions>,
    mode: FrameRateMode,
) {
    let Some(definition) = launch_options.and_then(|options| options.frame_rate.as_ref()) else {
        return;
    };
    let presets = definition.preset_values();
    let current = ui.get_game_frame_rate().trim().parse::<u32>().ok();
    let value = match mode {
        FrameRateMode::Auto => definition.auto.fallback,
        FrameRateMode::Preset => current
            .filter(|value| presets.contains(value))
            .unwrap_or(definition.auto.fallback),
        FrameRateMode::Custom => current.unwrap_or(definition.auto.fallback),
    };
    let preset_index = presets
        .iter()
        .position(|preset| *preset == value)
        .or_else(|| {
            presets
                .iter()
                .position(|preset| *preset == definition.auto.fallback)
        })
        .unwrap_or(0);

    ui.set_frame_rate_mode(mode.ui_index());
    ui.set_game_frame_rate(value.to_string().into());
    ui.set_frame_rate_preset_index(preset_index as f32);
    ui.set_frame_rate_notice(frame_rate_notice(mode, definition).into());
}

pub(crate) fn apply_frame_rate_preset_to_view(
    ui: &AppWindow,
    launch_options: Option<&ManifestLaunchOptions>,
    index: usize,
) {
    let Some(definition) = launch_options.and_then(|options| options.frame_rate.as_ref()) else {
        return;
    };
    let Some(value) = definition.preset_values().get(index).copied() else {
        return;
    };

    ui.set_frame_rate_mode(FrameRateMode::Preset.ui_index());
    ui.set_frame_rate_preset_index(index as f32);
    ui.set_game_frame_rate(value.to_string().into());
    ui.set_frame_rate_notice(frame_rate_notice(FrameRateMode::Preset, definition).into());
}

pub(crate) fn frame_rate_preference_from_view(
    ui: &AppWindow,
    launch_options: Option<&ManifestLaunchOptions>,
) -> Result<Option<FrameRatePreference>, String> {
    let Some(definition) = launch_options.and_then(|options| options.frame_rate.as_ref()) else {
        return Ok(None);
    };
    let mode = FrameRateMode::from_ui_index(ui.get_frame_rate_mode());
    if mode == FrameRateMode::Auto {
        return Ok(Some(FrameRatePreference::default()));
    }

    let value = ui
        .get_game_frame_rate()
        .trim()
        .parse::<u32>()
        .map_err(|_| {
            format!(
                "Use a whole number between {} and {}.",
                definition.custom_min, definition.custom_max
            )
        })?;
    if !(definition.custom_min..=definition.custom_max).contains(&value) {
        return Err(format!(
            "Use a whole number between {} and {}.",
            definition.custom_min, definition.custom_max
        ));
    }
    if mode == FrameRateMode::Preset && !definition.preset_values().contains(&value) {
        return Err("Select one of the frame-rate presets from the slider.".to_string());
    }

    Ok(Some(FrameRatePreference {
        mode,
        value: Some(value),
    }))
}

fn launch_options_view_state(
    config: &LauncherConfig,
    manifest_options: Option<&ManifestLaunchOptions>,
) -> (Vec<LaunchOptionView>, String) {
    let Some(manifest_options) = manifest_options else {
        return (Vec::new(), config.game_args.join(" "));
    };

    let (values, extra_args) = match config.launch_arguments_mode {
        LaunchArgumentsMode::GameDefaults => (
            manifest_options
                .game_arguments
                .iter()
                .map(|argument| argument.default)
                .collect(),
            config.game_args.clone(),
        ),
        LaunchArgumentsMode::Recommended => (
            manifest_options
                .game_arguments
                .iter()
                .map(|argument| argument.recommended.unwrap_or(argument.default))
                .collect(),
            config.game_args.clone(),
        ),
        LaunchArgumentsMode::Custom => parse_known_game_args(
            manifest_options,
            &config.game_args,
            default_launch_option_values(manifest_options),
        ),
    };

    (
        manifest_options
            .game_arguments
            .iter()
            .zip(values)
            .map(|(argument, checked)| LaunchOptionView {
                name: argument.name.clone().into(),
                flag: argument.flag.clone().into(),
                checked,
            })
            .collect(),
        quote_args_for_display(&extra_args),
    )
}

pub(crate) fn apply_launch_arguments_mode_to_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    manifest_options: Option<&ManifestLaunchOptions>,
    mode: LaunchArgumentsMode,
    extra_args: String,
) {
    ui.set_launch_arguments_mode(mode.ui_index());

    if matches!(mode, LaunchArgumentsMode::Custom) {
        return;
    }

    let preview_config = LauncherConfig {
        launch_arguments_mode: mode,
        game_args: config.game_args.clone(),
        ..config.clone()
    };
    let (options, _) = launch_options_view_state(&preview_config, manifest_options);
    apply_launch_options_to_view(ui, options, extra_args);
}

pub(crate) fn apply_launch_options_to_view(
    ui: &AppWindow,
    options: Vec<LaunchOptionView>,
    extra_args: String,
) {
    let state = launch_options_state(&options);
    ui.set_launch_option_min_width_px(132.0);
    ui.set_launch_options(ModelRc::new(VecModel::from(options)));
    ui.set_launch_options_state(state.into());
    ui.set_custom_game_args(extra_args.into());
}

pub(crate) fn launch_options_from_model(ui: &AppWindow) -> Vec<LaunchOptionView> {
    let model = ui.get_launch_options();
    (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .collect()
}

fn launch_options_state(options: &[LaunchOptionView]) -> String {
    options
        .iter()
        .map(|option| if option.checked { '1' } else { '0' })
        .collect()
}

pub(crate) fn launch_options_game_args(
    ui: &AppWindow,
    manifest_options: Option<&ManifestLaunchOptions>,
) -> Result<Vec<String>, String> {
    let mut args = if LaunchArgumentsMode::from_ui_index(ui.get_launch_arguments_mode())
        == LaunchArgumentsMode::Custom
    {
        known_launch_options_game_args(ui, manifest_options)
    } else {
        Vec::new()
    };
    let extra_args = game_launch::parse_command_line(ui.get_custom_game_args().trim())
        .map_err(|error| format!("Could not parse extra arguments: {error}"))?;
    if let Some(frame_rate) = manifest_options.and_then(|options| options.frame_rate.as_ref())
        && contains_controlled_argument(&extra_args, &frame_rate.flag)
    {
        return Err(format!(
            "Remove {} from Extra arguments and use the Frame rate control instead.",
            frame_rate.flag
        ));
    }
    args.extend(extra_args);
    Ok(args)
}

fn contains_controlled_argument(args: &[String], flag: &str) -> bool {
    let assignment_prefix = format!("{flag}=");
    args.iter()
        .any(|argument| argument == flag || argument.starts_with(&assignment_prefix))
}

fn known_launch_options_game_args(
    ui: &AppWindow,
    manifest_options: Option<&ManifestLaunchOptions>,
) -> Vec<String> {
    let Some(manifest_options) = manifest_options else {
        return Vec::new();
    };

    launch_options_from_model(ui)
        .into_iter()
        .zip(manifest_options.game_arguments.iter())
        .flat_map(|(option, argument)| {
            if option.checked == argument.default {
                Vec::new()
            } else {
                vec![argument.flag.clone(), option.checked.to_string()]
            }
        })
        .collect()
}

fn empty_launch_options_text(
    config: &LauncherConfig,
    launch_options: Option<&ManifestLaunchOptions>,
) -> &'static str {
    if launch_options.is_some() {
        return NO_KNOWN_LAUNCH_OPTIONS_TEXT;
    }

    let status = game_install::inspect_install(Some(&config.effective_install_dir()));
    if status.state == InstallState::NotInstalled {
        NO_INSTALLED_LAUNCH_OPTIONS_TEXT
    } else {
        NO_KNOWN_LAUNCH_OPTIONS_TEXT
    }
}

fn default_launch_option_values(launch_options: &ManifestLaunchOptions) -> Vec<bool> {
    launch_options
        .game_arguments
        .iter()
        .map(|argument| argument.default)
        .collect()
}

fn parse_known_game_args(
    launch_options: &ManifestLaunchOptions,
    args: &[String],
    mut values: Vec<bool>,
) -> (Vec<bool>, Vec<String>) {
    let mut extra_args = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let Some(option_index) = launch_options
            .game_arguments
            .iter()
            .position(|option| option.flag == *arg)
        else {
            extra_args.push(arg.clone());
            index += 1;
            continue;
        };

        let mut value = true;
        if let Some(next) = args.get(index + 1) {
            match next.to_ascii_lowercase().as_str() {
                "true" => {
                    value = true;
                    index += 1;
                }
                "false" => {
                    value = false;
                    index += 1;
                }
                _ => {}
            }
        }

        values[option_index] = value;
        index += 1;
    }

    (values, extra_args)
}

fn quote_args_for_display(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote_arg_for_display(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_arg_for_display(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }

    if !arg.chars().any(char::is_whitespace) && !arg.contains('"') && !arg.contains('\\') {
        return arg.to_string();
    }

    format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths;
    use crate::release_manifest::{ManifestAutoFrameRate, ManifestFrameRate};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn empty_launch_options_text_points_to_home_before_install() {
        let temp = tempdir().unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().join("missing-install")),
            ..LauncherConfig::default()
        };

        assert_eq!(
            empty_launch_options_text(&config, None),
            NO_INSTALLED_LAUNCH_OPTIONS_TEXT
        );
        assert!(!frame_rate_view_state(&config, None).controls_visible);
    }

    #[test]
    fn empty_launch_options_text_mentions_manifest_when_installed_without_options() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path();
        let game_dir = paths::game_dir(install_dir);
        fs::create_dir_all(&game_dir).unwrap();
        let executable = game_dir.join(game_install::game_executable_names()[0]);
        if cfg!(target_os = "macos") {
            fs::create_dir_all(&executable).unwrap();
        } else {
            fs::write(&executable, b"game").unwrap();
        }
        let config = LauncherConfig {
            install_dir: Some(install_dir.to_path_buf()),
            ..LauncherConfig::default()
        };

        assert_eq!(
            empty_launch_options_text(&config, None),
            NO_KNOWN_LAUNCH_OPTIONS_TEXT
        );
        let options = ManifestLaunchOptions {
            frame_rate: None,
            game_arguments: Vec::new(),
        };
        assert!(!has_known_launch_arguments(Some(&options)));
    }

    #[test]
    fn removed_preset_is_presented_as_a_preserved_custom_value() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path();
        let game_dir = paths::game_dir(install_dir);
        fs::create_dir_all(&game_dir).unwrap();
        let executable = game_dir.join(game_install::game_executable_names()[0]);
        if cfg!(target_os = "macos") {
            fs::create_dir_all(&executable).unwrap();
        } else {
            fs::write(&executable, b"game").unwrap();
        }
        let config = LauncherConfig {
            install_dir: Some(install_dir.to_path_buf()),
            frame_rate: FrameRatePreference {
                mode: FrameRateMode::Preset,
                value: Some(60),
            },
            ..LauncherConfig::default()
        };
        let options = ManifestLaunchOptions {
            frame_rate: Some(ManifestFrameRate {
                flag: "--fps".to_string(),
                auto: ManifestAutoFrameRate {
                    fallback: 120,
                    step: 24,
                    maximum: 144,
                },
                custom_min: 1,
                custom_max: 10_000,
            }),
            game_arguments: Vec::new(),
        };

        let state = frame_rate_view_state(&config, Some(&options));

        assert_eq!(state.mode, FrameRateMode::Custom);
        assert_eq!(state.value, 60);
    }

    #[test]
    fn old_manifest_uses_disabled_builtin_display_without_changing_config() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path();
        let game_dir = paths::game_dir(install_dir);
        fs::create_dir_all(&game_dir).unwrap();
        let executable = game_dir.join(game_install::game_executable_names()[0]);
        if cfg!(target_os = "macos") {
            fs::create_dir_all(&executable).unwrap();
        } else {
            fs::write(&executable, b"game").unwrap();
        }
        let config = LauncherConfig {
            install_dir: Some(install_dir.to_path_buf()),
            frame_rate: FrameRatePreference {
                mode: FrameRateMode::Custom,
                value: Some(300),
            },
            ..LauncherConfig::default()
        };

        let state = frame_rate_view_state(&config, None);

        assert!(state.controls_visible);
        assert!(!state.supported);
        assert_eq!(state.value, 0);
        assert_eq!(config.frame_rate.value, Some(300));
    }
}
