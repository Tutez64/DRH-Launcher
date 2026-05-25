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
        return dirs.data_local_dir().to_path_buf();
    }

    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("drh-launcher-data")
}

#[allow(dead_code)]
pub fn downloads_dir(install_dir: &std::path::Path) -> PathBuf {
    install_dir.join("launcher-data").join("downloads")
}

#[allow(dead_code)]
pub fn staging_dir(install_dir: &std::path::Path) -> PathBuf {
    install_dir.join("launcher-data").join("staging")
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io.github", "Tutez", "DRH Launcher")
}
