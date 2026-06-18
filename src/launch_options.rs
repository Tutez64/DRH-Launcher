use crate::config::{LaunchArgumentsMode, LauncherConfig};
use crate::release_manifest::ManifestLaunchOptions;
use crate::{AppWindow, LaunchOptionView, game_launch, install_metadata};
use slint::{Model, ModelRc, VecModel};

pub(crate) fn refresh_launch_options_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    launch_options: Option<&ManifestLaunchOptions>,
    save_text: &str,
) {
    let (view_options, custom_game_args) = launch_options_view_state(config, launch_options);
    let launch_options_state = launch_options_state(&view_options);

    ui.set_saved_pre_launch_command(config.pre_launch_command.clone().into());
    ui.set_saved_launch_arguments_mode(config.launch_arguments_mode.ui_index());
    ui.set_saved_custom_game_args(custom_game_args.clone().into());
    ui.set_saved_launch_options_state(launch_options_state.clone().into());

    ui.set_pre_launch_command(config.pre_launch_command.clone().into());
    ui.set_launch_arguments_mode(config.launch_arguments_mode.ui_index());
    ui.set_custom_game_args(custom_game_args.into());
    ui.set_launch_options_save_text(save_text.into());
    apply_launch_options_to_view(ui, view_options, ui.get_custom_game_args().to_string());
}

pub(crate) fn load_installed_launch_options(
    config: &LauncherConfig,
) -> Option<ManifestLaunchOptions> {
    let state = install_metadata::InstalledState::load(&config.effective_install_dir()).ok()?;
    state.active.launch_options
}

pub(crate) fn recommended_game_args_from_launch_options(
    launch_options: Option<&ManifestLaunchOptions>,
) -> Vec<String> {
    launch_options
        .map(recommended_launch_option_args)
        .unwrap_or_default()
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
    args.extend(extra_args);
    Ok(args)
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
