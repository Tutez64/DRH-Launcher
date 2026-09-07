use std::sync::Mutex;

use crate::config::LauncherConfig;
use crate::github_releases::PlatformRelease;
use crate::home_notices;
use crate::install_state::InstallState;
use crate::paths;
use crate::steam_buildid;
use crate::steam_players;
use crate::{
    AppWindow, game_install, install_metadata, release_update_available,
    rollback_blocked_update_version,
};

static LATEST_KNOWN_DRH_RELEASE: Mutex<Option<PlatformRelease>> = Mutex::new(None);

#[cfg(test)]
static LATEST_RELEASE_TEST_LOCK: Mutex<()> = Mutex::new(());

const HOME_ERROR_MAX_LEN: usize = 80;
const HOME_ERROR_LOGS_SUFFIX: &str = " See Settings → Logs for details.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HomeMessage {
    Ready,
    Running,
    Progress(String),
    Installed { version: String },
    Stopped,
    Exited,
    ConfigWarning(String),
    Notice(String),
    Error(String),
    UpdateCheckFailed(String),
}

impl HomeMessage {
    pub(crate) fn progress(text: impl Into<String>) -> Self {
        Self::Progress(text.into())
    }

    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self::Error(text.into())
    }

    pub(crate) fn notice(text: impl Into<String>) -> Self {
        Self::Notice(text.into())
    }

    fn support_text(&self, _version_text: &str) -> Option<String> {
        match self {
            Self::Ready | Self::Running => None,
            Self::Progress(text) => Some(text.clone()),
            Self::Installed { version } => Some(format!("Installed {version}.")),
            Self::Stopped => Some("DRH has stopped.".to_string()),
            Self::Exited => Some("DRH exited.".to_string()),
            Self::ConfigWarning(warning) => Some(format!("Configuration warning: {warning}")),
            Self::Notice(text) => Some(text.clone()),
            Self::Error(text) | Self::UpdateCheckFailed(text) => {
                Some(format_home_error_message(text))
            }
        }
    }

    fn uses_release_overlay(&self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Running | Self::Installed { .. } | Self::Stopped | Self::Exited
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HomeActivity {
    Idle,
    CheckingUpdates,
    Updating,
    Playing { stopping: bool },
}

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

pub(crate) fn refresh_home_state(ui: &AppWindow, config: &LauncherConfig, message: HomeMessage) {
    let state = home_view_state_from_cache(config, HomeActivity::Idle, &message);
    apply_home_view_state(ui, state);
    home_notices::apply_home_notices_view(ui, config);
    apply_player_count_view(ui);
}

pub(crate) fn apply_updating_home_state(
    ui: &AppWindow,
    config: &LauncherConfig,
    message: impl Into<String>,
) {
    let state = home_view_state_from_cache(
        config,
        HomeActivity::Updating,
        &HomeMessage::progress(message),
    );
    apply_home_view_state(ui, state);
    home_notices::apply_home_notices_view(ui, config);
    apply_player_count_view(ui);
}

pub(crate) fn apply_playing_home_state(
    ui: &AppWindow,
    config: &LauncherConfig,
    message: HomeMessage,
    stopping: bool,
) {
    let state = home_view_state_from_cache(config, HomeActivity::Playing { stopping }, &message);
    apply_home_view_state(ui, state);
    home_notices::apply_home_notices_view(ui, config);
    apply_player_count_view(ui);
}

pub(crate) fn remember_latest_drh_release(release: &PlatformRelease) {
    let version = release.version.trim();
    if version.is_empty() {
        return;
    }
    *lock_latest_known_drh_release() = Some(release.clone());
}

pub(crate) fn home_view_state_from_cache(
    config: &LauncherConfig,
    activity: HomeActivity,
    message: &HomeMessage,
) -> HomeViewState {
    let latest_release = cached_latest_drh_release();
    home_view_state(config, latest_release.as_ref(), activity, message)
}

pub(crate) fn official_update_warning(config: &LauncherConfig) -> String {
    steam_buildid::official_update_text(
        steam_buildid::installed_release_steam_buildid(config),
        steam_buildid::cached_official_public_buildid(),
        installed_active_release_version(config).as_deref(),
        latest_known_drh_version().as_deref(),
    )
}

pub(crate) fn apply_player_count_view(ui: &AppWindow) {
    match steam_players::cached_player_count() {
        Some(count) => {
            ui.set_steam_players_count(count.to_string().into());
            ui.set_steam_players_detail(steam_players::players_detail_text(count).into());
        }
        None => {
            ui.set_steam_players_count(String::new().into());
            ui.set_steam_players_detail(String::new().into());
        }
    }
}

fn latest_known_drh_version() -> Option<String> {
    cached_latest_drh_release().map(|release| release.version)
}

pub(crate) fn cached_latest_drh_release() -> Option<PlatformRelease> {
    lock_latest_known_drh_release().clone()
}

fn lock_latest_known_drh_release() -> std::sync::MutexGuard<'static, Option<PlatformRelease>> {
    LATEST_KNOWN_DRH_RELEASE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    activity: HomeActivity,
    message: &HomeMessage,
) -> HomeViewState {
    let install_dir = config.effective_install_dir();
    let status = game_install::inspect_install(Some(&install_dir));
    let version_status = status.version_text();

    let mut state = HomeViewState {
        install_status: status.status_text(),
        install_action_text: status.state.primary_action().to_string(),
        install_action_enabled: true,
        version_status: version_status.clone(),
        home_support_text: version_status.clone(),
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

    let show_release_overlay =
        matches!(activity, HomeActivity::Idle | HomeActivity::CheckingUpdates)
            && message.uses_release_overlay();

    let mut used_overlay = false;
    if show_release_overlay && let Some(release) = latest_release {
        apply_release_to_home_view_state(&mut state, config, &status, release);
        used_overlay = true;
    }

    if let HomeMessage::UpdateCheckFailed(error) = message
        && status.state == InstallState::Installed
    {
        state.install_status = InstallState::LaunchableButMaybeOutdated
            .status_text()
            .to_string();
        state.install_action_text = InstallState::LaunchableButMaybeOutdated
            .primary_action()
            .to_string();
        state.home_support_text = format_home_error_message(error);
        used_overlay = true;
    }

    if !used_overlay {
        if let Some(text) = message.support_text(&version_status) {
            state.home_support_text = text;
        } else if *message == HomeMessage::Ready
            && let Some(reason) = status.reason.as_deref().filter(|reason| !reason.is_empty())
        {
            state.home_support_text = format_home_error_message(reason);
        }
    }

    apply_activity(&mut state, activity);
    state
}

fn apply_activity(state: &mut HomeViewState, activity: HomeActivity) {
    match activity {
        HomeActivity::Idle => {}
        HomeActivity::CheckingUpdates => {
            state.update_check_enabled = false;
            state.update_check_text = "Checking...".to_string();
        }
        HomeActivity::Updating => {
            state.install_status = InstallState::Updating.status_text().to_string();
            state.install_action_text = InstallState::Updating.primary_action().to_string();
            state.install_action_enabled = false;
            state.update_check_enabled = false;
            state.restore_previous_enabled = false;
            state.reinstall_current_enabled = false;
        }
        HomeActivity::Playing { stopping } => {
            state.install_status = InstallState::Playing.status_text().to_string();
            state.install_action_text = InstallState::Playing.primary_action().to_string();
            state.install_action_enabled = !stopping;
            state.update_check_enabled = false;
            state.restore_previous_enabled = false;
            state.reinstall_current_enabled = false;
        }
    }
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
                    state.install_status = InstallState::UpdateAvailable.status_text().to_string();
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

pub(crate) fn set_status_message(ui: &AppWindow, message: HomeMessage) {
    let version_text = ui.get_version_status();
    let text = message
        .support_text(&version_text)
        .unwrap_or(version_text.to_string());
    ui.set_home_support_text(text.into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github_releases::{ReleaseAsset, ReleaseMetadataSource};
    use crate::install_metadata::{InstalledRelease, InstalledState};
    use std::fs;
    use tempfile::tempdir;

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
            HomeMessage::error(message)
                .support_text("Version: V1")
                .as_deref(),
            Some("Could not open logs folder: denied. See Settings → Logs for details.")
        );
    }

    #[test]
    fn ready_and_progress_messages_do_not_use_prefix_matching() {
        assert_eq!(HomeMessage::Ready.support_text("Version: V1"), None);
        assert_eq!(
            HomeMessage::progress("Extracting archive...").support_text("Version: V1"),
            Some("Extracting archive...".to_string())
        );
        assert_eq!(
            HomeMessage::Installed {
                version: "V2".to_string()
            }
            .support_text("Version: V1"),
            Some("Installed V2.".to_string())
        );
    }

    #[test]
    fn cached_latest_release_keeps_update_available_after_refresh() {
        let _test_guard = LATEST_RELEASE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *lock_latest_known_drh_release() = None;

        let temp = tempdir().unwrap();
        let game_dir = paths::game_dir(temp.path());
        fs::create_dir_all(&game_dir).unwrap();
        let executable = game_dir.join(game_install::game_executable_names()[0]);
        if cfg!(target_os = "macos") {
            fs::create_dir_all(&executable).unwrap();
        } else {
            fs::write(&executable, "").unwrap();
        }
        InstalledState {
            active: InstalledRelease {
                version: "V9".to_string(),
                platform: "linux-x64".to_string(),
                source: "Tutez64/DRHL-Release-Fixtures".to_string(),
                release_url: "https://example.test/V9".to_string(),
                archive: "archive-V9.tar.gz".to_string(),
                archive_sha256: "abc123".to_string(),
                archive_size: 123,
                installed_at: "unix:0".to_string(),
                launch_options: None,
                steam_buildid: None,
            },
            previous: None,
            blocked_update_version: None,
        }
        .save(temp.path())
        .unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };

        let without_cache =
            home_view_state_from_cache(&config, HomeActivity::Idle, &HomeMessage::Ready);
        assert_eq!(without_cache.install_action_text, "Play");

        remember_latest_drh_release(&test_release("V10"));
        let with_cache =
            home_view_state_from_cache(&config, HomeActivity::Idle, &HomeMessage::Ready);
        assert_eq!(with_cache.install_action_text, "Update");
        assert_eq!(
            with_cache.install_status,
            InstallState::UpdateAvailable.status_text()
        );

        let error_state = home_view_state_from_cache(
            &config,
            HomeActivity::Idle,
            &HomeMessage::error("Could not restore previous version: denied"),
        );
        assert!(
            error_state
                .home_support_text
                .starts_with("Could not restore previous version")
        );
        assert!(!error_state.home_support_text.contains("Update available"));

        let failed_check = home_view_state_from_cache(
            &config,
            HomeActivity::Idle,
            &HomeMessage::UpdateCheckFailed(
                "Could not check GitHub releases: connection reset".to_string(),
            ),
        );
        assert_eq!(
            failed_check.install_status,
            InstallState::LaunchableButMaybeOutdated.status_text()
        );
        assert_eq!(
            failed_check.install_action_text,
            InstallState::LaunchableButMaybeOutdated.primary_action()
        );
        assert!(
            failed_check
                .home_support_text
                .contains("Could not check GitHub releases")
        );
        assert!(!failed_check.home_support_text.contains("Update available"));

        *lock_latest_known_drh_release() = None;
    }

    fn test_release(version: &str) -> PlatformRelease {
        PlatformRelease {
            version: version.to_string(),
            name: format!("Dungeon Rampage Haxe {version}"),
            html_url: format!("https://example.test/{version}"),
            metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
            launch_options: None,
            steam_buildid: None,
            asset: ReleaseAsset {
                platform_id: "linux-x64".to_string(),
                name: format!("Dungeon.Rampage.Haxe.{version}.Linux.tar.gz"),
                download_url: "https://example.test/archive.tar.gz".to_string(),
                size: 123,
                digest: Some("sha256:abc123".to_string()),
            },
        }
    }
}
