use directories::ProjectDirs;
use std::env;
use std::path::PathBuf;

pub fn config_file() -> PathBuf {
    project_dirs()
        .map(|dirs| dirs.config_dir().join("config.json"))
        .unwrap_or_else(|| PathBuf::from("config.json"))
}

pub fn default_install_dir() -> PathBuf {
    if let Some(dirs) = project_dirs() {
        return dirs.data_local_dir().join("game");
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("game")
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io.github", "Tutez", "DRH Launcher")
}
