use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use crate::config::LauncherConfig;
use crate::game_install::{game_executable_names, is_game_executable};
use crate::paths;

pub fn launch_game(config: &LauncherConfig) -> Result<Child, String> {
    let install_dir = config
        .install_dir
        .as_deref()
        .ok_or_else(|| "No install directory selected.".to_string())?;
    let game_dir = paths::game_dir(install_dir);
    let executable = find_game_executable(&game_dir)?;

    let mut command = if cfg!(target_os = "macos") && executable.is_dir() {
        let mut command = Command::new("open");
        command.arg(&executable);
        if !config.game_args.is_empty() {
            command.arg("--args").args(&config.game_args);
        }
        command
    } else {
        let mut command = Command::new(&executable);
        command.args(&config.game_args);
        command
    };

    command.current_dir(&game_dir);
    command
        .spawn()
        .map_err(|error| format!("Could not launch DRH: {error}"))
}

pub fn find_game_executable(game_dir: &Path) -> Result<PathBuf, String> {
    game_executable_names()
        .iter()
        .map(|name| game_dir.join(name))
        .find(|path| is_game_executable(path))
        .ok_or_else(|| {
            format!(
                "Could not find game executable. Tried: {}",
                game_executable_names().join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn finds_primary_game_executable() {
        let temp = tempdir().unwrap();
        create_executable(&temp.path().join(game_executable_names()[0]));

        let executable = find_game_executable(temp.path()).unwrap();

        assert_eq!(executable, temp.path().join(game_executable_names()[0]));
    }

    #[test]
    fn finds_legacy_game_executable() {
        let temp = tempdir().unwrap();
        create_executable(&temp.path().join(game_executable_names()[1]));

        let executable = find_game_executable(temp.path()).unwrap();

        assert_eq!(executable, temp.path().join(game_executable_names()[1]));
    }

    #[test]
    fn reports_missing_game_executable() {
        let temp = tempdir().unwrap();

        let error = find_game_executable(temp.path()).unwrap_err();

        assert!(error.contains("Could not find game executable"));
    }

    fn create_executable(path: &Path) {
        if cfg!(target_os = "macos") {
            fs::create_dir_all(path).unwrap();
        } else {
            fs::write(path, "").unwrap();
        }
    }
}
