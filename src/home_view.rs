use crate::config::LauncherConfig;
use crate::diagnostics;
use crate::github_releases::PlatformRelease;
use crate::install_state::InstallState;
use crate::paths;
use crate::{
    AppWindow, game_install, install_metadata, release_update_available,
    rollback_blocked_update_version,
};

const HOME_ERROR_MAX_LEN: usize = 80;
const HOME_ERROR_LOGS_SUFFIX: &str = " See Settings → Logs for details.";

pub(crate) struct HomeViewState {
    pub(crate) install_status: String,
    pub(crate) install_action_text: String,
    pub(crate) install_action_enabled: bool,
    pub(crate) version_status: String,
    pub(crate) home_support_text: String,
    pub(crate) update_check_text: String,
    pub(crate) update_check_enabled: bool,
    pub(crate) open_install_folder_enabled: bool,
    pub(crate) open_logs_folder_enabled: bool,
    pub(crate) restore_previous_enabled: bool,
    pub(crate) restore_previous_text: String,
    pub(crate) reinstall_current_enabled: bool,
    pub(crate) reinstall_current_text: String,
}

pub(crate) fn refresh_home_state(ui: &AppWindow, config: &LauncherConfig, message: &str) {
    let state = home_view_state(config, None, message);
    apply_home_view_state(ui, state);
}

pub(crate) fn apply_updating_home_state(
    ui: &AppWindow,
    config: &LauncherConfig,
    message: &str,
) {
    let version_text = game_install::inspect_install(Some(&config.effective_install_dir()))
        .version_text();
    ui.set_install_status(InstallState::Updating.status_text().into());
    ui.set_version_status(version_text.clone().into());
    ui.set_home_support_text(home_support_text(&version_text, message).into());
    ui.set_install_action_text(InstallState::Updating.primary_action().into());
    ui.set_install_action_enabled(false);
    ui.set_update_check_enabled(false);
    ui.set_restore_previous_enabled(false);
    ui.set_reinstall_current_enabled(false);
}

pub(crate) fn restore_previous_version_text(config: &LauncherConfig) -> String {
    restore_previous_release_version(config)
        .map(|version| format!("Restore {version}"))
        .unwrap_or_else(|| "Restore previous".to_string())
}

pub(crate) fn restore_previous_version_available(config: &LauncherConfig) -> bool {
    let install_dir = config.effective_install_dir();

    restore_previous_release_version(config).is_some()
        && paths::previous_game_dir(&install_dir).exists()
}

pub(crate) fn restore_previous_release_version(config: &LauncherConfig) -> Option<String> {
    let state = install_metadata::InstalledState::load(&config.effective_install_dir()).ok()?;
    state.previous.map(|previous| previous.version)
}

fn reinstall_current_version_text(config: &LauncherConfig) -> String {
    installed_active_release_version(config)
        .map(|version| format!("Reinstall {version}"))
        .unwrap_or_else(|| "Reinstall current".to_string())
}

fn reinstall_current_version_available(config: &LauncherConfig) -> bool {
    installed_active_release_version(config).is_some()
}

pub(crate) fn installed_active_release_version(config: &LauncherConfig) -> Option<String> {
    install_metadata::InstalledState::load(&config.effective_install_dir())
        .ok()
        .map(|state| state.active.version)
}

pub(crate) fn home_view_state(
    config: &LauncherConfig,
    latest_release: Option<&PlatformRelease>,
    message: &str,
) -> HomeViewState {
    let install_dir = config.effective_install_dir();
    let status = game_install::inspect_install(Some(&install_dir));
    let version_status = status.version_text();
    let activity_message = status
        .reason
        .as_deref()
        .filter(|reason| !reason.is_empty())
        .filter(|_| message == "Ready.")
        .unwrap_or(message);

    let mut state = HomeViewState {
        install_status: status.status_text(),
        install_action_text: status.state.primary_action().to_string(),
        install_action_enabled: true,
        version_status: version_status.clone(),
        home_support_text: home_support_text(&version_status, activity_message),
        update_check_text: "Check for updates".to_string(),
        update_check_enabled: true,
        open_install_folder_enabled: install_dir.exists(),
        open_logs_folder_enabled: paths::logs_dir(&install_dir).exists()
            || paths::launcher_log_file(&install_dir).exists(),
        restore_previous_enabled: restore_previous_version_available(config),
        restore_previous_text: restore_previous_version_text(config),
        reinstall_current_enabled: reinstall_current_version_available(config),
        reinstall_current_text: reinstall_current_version_text(config),
    };

    if let Some(release) = latest_release {
        apply_release_to_home_view_state(&mut state, config, &status, release);
    }

    state
}

fn apply_release_to_home_view_state(
    state: &mut HomeViewState,
    config: &LauncherConfig,
    status: &game_install::InstallStatus,
    release: &PlatformRelease,
) {
    match status.state {
        InstallState::Installed | InstallState::LaunchableButMaybeOutdated => {
            match status.installed_version.as_deref() {
                Some(installed_version) if release_update_available(config, status, release) => {
                    state.install_status = "A DRH update is available".to_string();
                    state.install_action_text =
                        InstallState::UpdateAvailable.primary_action().to_string();
                    state.home_support_text = format!(
                        "Update available: installed {installed_version}, latest {}.",
                        release.version
                    );
                }
                Some(installed_version)
                    if rollback_blocked_update_version(config).as_deref()
                        == Some(release.version.as_str()) =>
                {
                    state.home_support_text = format!(
                        "You restored {installed_version}. Update {} is skipped until a newer release is available.",
                        release.version
                    );
                }
                Some(installed_version) => {
                    state.home_support_text = format!("You are up to date: {installed_version}.");
                }
                None => {
                    state.home_support_text = format!(
                        "Latest available version: {}. Installed version unknown.",
                        release.version
                    );
                }
            }
        }
        InstallState::NotInstalled => {
            state.home_support_text = format!("Latest available version: {}.", release.version);
        }
        InstallState::BrokenInstall => {
            state.home_support_text = format!(
                "Latest available version: {}. Repair is required.",
                release.version
            );
        }
        InstallState::UpdateAvailable | InstallState::Updating | InstallState::Playing => {}
    }
}

pub(crate) fn apply_home_view_state(ui: &AppWindow, state: HomeViewState) {
    ui.set_install_status(state.install_status.into());
    ui.set_install_action_text(state.install_action_text.into());
    ui.set_install_action_enabled(state.install_action_enabled);
    ui.set_version_status(state.version_status.into());
    ui.set_home_support_text(state.home_support_text.into());
    ui.set_update_check_text(state.update_check_text.into());
    ui.set_update_check_enabled(state.update_check_enabled);
    ui.set_open_install_folder_enabled(state.open_install_folder_enabled);
    ui.set_open_logs_folder_enabled(state.open_logs_folder_enabled);
    ui.set_restore_previous_enabled(state.restore_previous_enabled);
    ui.set_restore_previous_text(state.restore_previous_text.into());
    ui.set_reinstall_current_enabled(state.reinstall_current_enabled);
    ui.set_reinstall_current_text(state.reinstall_current_text.into());
}

pub(crate) fn set_status_message(ui: &AppWindow, message: &str) {
    let version_text = ui.get_version_status();
    ui.set_home_support_text(home_support_text(&version_text, message).into());
}

pub(crate) fn home_support_text(version_text: &str, message: &str) -> String {
    if message.starts_with("Ready.") || message.starts_with("DRH is running.") {
        return version_text.to_string();
    }

    if message.starts_with("Configuration warning: ") {
        return message.to_string();
    }

    if message.starts_with("DRH is not installed yet.")
        || message.starts_with("DRH cannot be launched from --play:")
        || message.starts_with("DRH update available before --play:")
    {
        return message.to_string();
    }

    if is_progress_message(message) {
        return message.to_string();
    }

    if let Some(installed_version) = message
        .strip_prefix("Installed ")
        .and_then(|message| message.split_once('.').map(|(version, _)| version))
    {
        return format!("Installed {installed_version}.");
    }

    if message.starts_with("Latest known version: ") {
        return message.to_string();
    }

    if message.starts_with("DRH stopped.") {
        return "DRH has stopped.".to_string();
    }

    if message.starts_with("DRH exited ") {
        return "DRH exited.".to_string();
    }

    if diagnostics::is_operation_error_message(message) {
        return format_home_error_message(message);
    }

    version_text.to_string()
}

fn format_home_error_message(message: &str) -> String {
    let available = HOME_ERROR_MAX_LEN.saturating_sub(HOME_ERROR_LOGS_SUFFIX.len() + 1);
    let (body, ellipsis) = if message.len() <= available {
        (message, "")
    } else {
        let truncated = truncate_at_word_boundary(message, available);
        (truncated, "…")
    };
    let separator = if body.ends_with(['.', '!', '?']) || !ellipsis.is_empty() {
        ""
    } else {
        "."
    };
    format!("{body}{ellipsis}{separator}{HOME_ERROR_LOGS_SUFFIX}")
}

fn truncate_at_word_boundary(message: &str, max_len: usize) -> &str {
    if message.len() <= max_len {
        return message;
    }

    let mut end = max_len;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }

    message[..end]
        .rfind([' ', ',', ';', ':'])
        .map(|index| &message[..index])
        .unwrap_or(&message[..end])
}

fn is_progress_message(message: &str) -> bool {
    message.starts_with("Checking ")
        || message.starts_with("Preparing download ")
        || message.starts_with("Downloading ")
        || message.starts_with("Verifying download")
        || message.starts_with("Extracting archive")
        || message.starts_with("Installing files")
        || message.starts_with("Installing ")
        || message.starts_with("Repairing ")
        || message.starts_with("Reinstalling ")
        || message.starts_with("Stopping ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_long_home_errors_and_points_to_logs() {
        let message = "Could not download a very long archive name that should not take over the entire home support area of the launcher UI";

        let formatted = format_home_error_message(message);

        assert!(formatted.ends_with(HOME_ERROR_LOGS_SUFFIX));
        assert!(formatted.starts_with("Could not download a very long archive name"));
        assert!(formatted.contains('…'));
        assert!(formatted.len() <= HOME_ERROR_MAX_LEN + HOME_ERROR_LOGS_SUFFIX.len() + 4);
    }

    #[test]
    fn keeps_short_home_errors_with_logs_suffix() {
        let message = "Could not open logs folder: denied";

        assert_eq!(
            home_support_text("Version: V1", message),
            format!("{message}.{HOME_ERROR_LOGS_SUFFIX}")
        );
    }
}
