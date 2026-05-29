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
        .join("drh-launcher")
}

pub fn downloads_dir(install_dir: &std::path::Path) -> PathBuf {
    data_dir(install_dir).join("downloads")
}

pub fn download_cache_index_file(install_dir: &std::path::Path) -> PathBuf {
    downloads_dir(install_dir).join("cache.txt")
}

pub fn staging_dir(install_dir: &std::path::Path) -> PathBuf {
    data_dir(install_dir).join("staging")
}

pub fn data_dir(install_dir: &std::path::Path) -> PathBuf {
    install_dir.join("data")
}

pub fn logs_dir(install_dir: &std::path::Path) -> PathBuf {
    data_dir(install_dir).join("logs")
}

pub fn launcher_log_file(install_dir: &std::path::Path) -> PathBuf {
    logs_dir(install_dir).join("launcher.log")
}

pub fn game_root_dir(install_dir: &std::path::Path) -> PathBuf {
    install_dir.join("Dungeon Rampage Haxe")
}

pub fn game_dir(install_dir: &std::path::Path) -> PathBuf {
    game_root_dir(install_dir).join("current")
}

pub fn previous_game_dir(install_dir: &std::path::Path) -> PathBuf {
    game_root_dir(install_dir).join("previous")
}

pub fn installed_metadata_file(install_dir: &std::path::Path) -> PathBuf {
    data_dir(install_dir).join("installed.json")
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("io.github", "Tutez", "DRH Launcher")
}
