use std::path::{Path, PathBuf};

use crate::install_metadata::InstalledState;
use crate::install_state::InstallState;
use crate::paths;

#[derive(Clone, Debug)]
pub struct InstallStatus {
    pub state: InstallState,
    pub install_dir: Option<PathBuf>,
    pub installed_version: Option<String>,
    pub reason: Option<String>,
}

impl InstallStatus {
    pub fn not_installed() -> Self {
        Self {
            state: InstallState::NotInstalled,
            install_dir: None,
            installed_version: None,
            reason: None,
        }
    }

    pub fn status_text(&self) -> String {
        match self.state {
            InstallState::NotInstalled => "DRH is not installed".to_string(),
            InstallState::Installed => "DRH is installed".to_string(),
            InstallState::UpdateAvailable => "A DRH update is available".to_string(),
            InstallState::Updating => "DRH is updating".to_string(),
            InstallState::Playing => "DRH is running".to_string(),
            InstallState::BrokenInstall => "DRH installation is incomplete".to_string(),
            InstallState::LaunchableButMaybeOutdated => {
                "DRH is launchable, but update status is unknown".to_string()
            }
        }
    }

    pub fn version_text(&self) -> String {
        self.installed_version
            .as_deref()
            .map(|version| format!("Version: {version}"))
            .unwrap_or_else(|| "Version: unknown".to_string())
    }
}

pub fn inspect_install(install_dir: Option<&Path>) -> InstallStatus {
    let Some(install_dir) = install_dir else {
        return InstallStatus::not_installed();
    };

    if !install_dir.exists() {
        return InstallStatus {
            state: InstallState::NotInstalled,
            install_dir: Some(install_dir.to_path_buf()),
            installed_version: None,
            reason: Some("Install directory does not exist.".to_string()),
        };
    }

    let game_dir = paths::game_dir(install_dir);
    if !game_dir.exists() {
        return InstallStatus {
            state: InstallState::NotInstalled,
            install_dir: Some(install_dir.to_path_buf()),
            installed_version: None,
            reason: Some("Game directory does not exist.".to_string()),
        };
    }

    let installed_state = InstalledState::load(install_dir).ok();
    let installed_version = installed_state
        .as_ref()
        .map(|state| state.active.version.clone());

    let executable_path = game_executable_names()
        .iter()
        .map(|name| game_dir.join(name))
        .find(|path| is_game_executable(path));
    if executable_path.is_none() {
        return InstallStatus {
            state: InstallState::BrokenInstall,
            install_dir: Some(install_dir.to_path_buf()),
            installed_version,
            reason: Some(format!(
                "Expected game executable is missing. Tried: {}",
                game_executable_names().join(", ")
            )),
        };
    }

    InstallStatus {
        state: InstallState::Installed,
        install_dir: Some(install_dir.to_path_buf()),
        installed_version,
        reason: None,
    }
}

pub fn game_executable_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["Dungeon Rampage Haxe.exe", "DungeonBustersProject.exe"]
    } else if cfg!(target_os = "macos") {
        &["Dungeon Rampage Haxe.app", "DungeonBustersProject.app"]
    } else {
        &["Dungeon Rampage Haxe", "DungeonBustersProject"]
    }
}

pub fn is_game_executable(path: &Path) -> bool {
    if cfg!(target_os = "macos") {
        path.is_dir()
    } else {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn reports_not_installed_without_install_directory() {
        let status = inspect_install(None);

        assert_eq!(status.state, InstallState::NotInstalled);
        assert!(status.installed_version.is_none());
    }

    #[test]
    fn reports_not_installed_when_game_directory_is_missing() {
        let temp = tempdir().unwrap();
        let status = inspect_install(Some(temp.path()));

        assert_eq!(status.state, InstallState::NotInstalled);
        assert_eq!(
            status.reason.as_deref(),
            Some("Game directory does not exist.")
        );
    }

    #[test]
    fn reports_installed_with_unknown_version_when_metadata_is_missing() {
        let temp = tempdir().unwrap();
        let game_dir = paths::game_dir(temp.path());
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join(primary_game_executable_name()), "").unwrap();

        let status = inspect_install(Some(temp.path()));

        assert_eq!(status.state, InstallState::Installed);
        assert!(status.installed_version.is_none());
    }

    #[test]
    fn reports_broken_install_when_executable_is_missing() {
        let temp = tempdir().unwrap();
        let game_dir = paths::game_dir(temp.path());
        fs::create_dir_all(&game_dir).unwrap();

        let status = inspect_install(Some(temp.path()));

        assert_eq!(status.state, InstallState::BrokenInstall);
        assert_eq!(status.installed_version.as_deref(), None);
        assert!(
            status
                .reason
                .as_deref()
                .unwrap()
                .contains("Expected game executable is missing")
        );
    }

    #[test]
    fn reports_installed_when_required_files_exist() {
        let temp = tempdir().unwrap();
        let game_dir = paths::game_dir(temp.path());
        fs::create_dir_all(&game_dir).unwrap();
        fs::write(game_dir.join(primary_game_executable_name()), "").unwrap();

        let status = inspect_install(Some(temp.path()));

        assert_eq!(status.state, InstallState::Installed);
        assert_eq!(status.installed_version.as_deref(), None);
        assert!(status.reason.is_none());
    }

    fn primary_game_executable_name() -> &'static str {
        game_executable_names()[0]
    }
}
