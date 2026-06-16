use cargo_packager_updater::{Config, Update, semver::Version, url::Url};
use std::env;
use std::process::Command;
use std::time::Duration;

use crate::linux_appimage;

const DEFAULT_UPDATE_ENDPOINT: &str =
    "https://github.com/Tutez64/DRH-Launcher/releases/latest/download/latest.json";

#[derive(Clone, Debug)]
pub struct LauncherUpdate {
    update: Update,
}

impl LauncherUpdate {
    pub fn version(&self) -> &str {
        &self.update.version
    }

    pub fn install_and_restart(&self) -> Result<(), String> {
        if !automatic_update_supported() {
            return Err(
                "Automatic launcher updates require a managed AppImage, macOS app, or Windows package."
                    .to_string(),
            );
        }

        let restart_command = restart_command()?;
        self.update
            .download_and_install()
            .map_err(|error| format!("Could not install launcher update: {error}"))?;

        Command::new(restart_command)
            .spawn()
            .map_err(|error| format!("Launcher updated, but could not restart it: {error}"))?;
        std::process::exit(0);
    }
}

pub fn automatic_updates_configured() -> bool {
    !public_key().is_empty()
}

pub fn automatic_update_supported() -> bool {
    if cfg!(debug_assertions) || !automatic_updates_configured() {
        return false;
    }

    if cfg!(target_os = "windows") {
        return !cfg!(debug_assertions);
    }

    if cfg!(target_os = "macos") {
        return env::current_exe()
            .ok()
            .is_some_and(|path| path.to_string_lossy().contains(".app/Contents/MacOS/"));
    }

    linux_appimage::is_managed_install()
}

pub fn check() -> Result<Option<LauncherUpdate>, String> {
    if !automatic_updates_configured() {
        return Err("Automatic launcher updates are not configured in this build.".to_string());
    }

    if !automatic_update_supported() {
        return Err(
            "Automatic launcher updates are unavailable for this installation format.".to_string(),
        );
    }

    let current_version = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("Invalid launcher version: {error}"))?;
    let runtime_update_endpoint = env::var("DRHL_UPDATE_ENDPOINT").ok();
    let endpoint = Url::parse(configured_update_endpoint(
        runtime_update_endpoint.as_deref(),
        option_env!("DRHL_UPDATE_ENDPOINT"),
    ))
    .map_err(|error| format!("Invalid launcher update endpoint: {error}"))?;
    let config = Config {
        endpoints: vec![endpoint],
        pubkey: public_key().to_string(),
        ..Default::default()
    };
    let updater = cargo_packager_updater::UpdaterBuilder::new(current_version, config)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not configure launcher updates: {error}"))?;

    updater
        .check()
        .map(|update| update.map(|update| LauncherUpdate { update }))
        .map_err(|error| format!("Could not check launcher updates: {error}"))
}

fn public_key() -> &'static str {
    option_env!("DRHL_UPDATE_PUBLIC_KEY").unwrap_or("").trim()
}

fn configured_update_endpoint<'a>(
    runtime_value: Option<&'a str>,
    compile_time_value: Option<&'a str>,
) -> &'a str {
    non_empty_trimmed(runtime_value)
        .or_else(|| non_empty_trimmed(compile_time_value))
        .unwrap_or(DEFAULT_UPDATE_ENDPOINT)
}

fn non_empty_trimmed(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    if value.is_empty() { None } else { Some(value) }
}

fn restart_command() -> Result<std::path::PathBuf, String> {
    if cfg!(target_os = "linux") {
        return env::var_os("APPIMAGE")
            .map(Into::into)
            .ok_or_else(|| "Could not locate the installed AppImage.".to_string());
    }

    env::current_exe().map_err(|error| format!("Could not locate DRH Launcher: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_build_does_not_claim_installed_update_support() {
        if cfg!(debug_assertions) {
            assert!(!automatic_update_supported());
        }
    }

    #[test]
    fn update_endpoint_defaults_to_latest_github_release() {
        assert_eq!(
            configured_update_endpoint(None, None),
            DEFAULT_UPDATE_ENDPOINT
        );
        assert_eq!(
            configured_update_endpoint(Some(""), Some("")),
            DEFAULT_UPDATE_ENDPOINT
        );
        assert_eq!(
            configured_update_endpoint(Some("  \n\t  "), None),
            DEFAULT_UPDATE_ENDPOINT
        );
    }

    #[test]
    fn update_endpoint_can_be_overridden_at_compile_time() {
        assert_eq!(
            configured_update_endpoint(None, Some("  https://example.test/latest.json  ")),
            "https://example.test/latest.json"
        );
    }

    #[test]
    fn update_endpoint_can_be_overridden_at_runtime() {
        assert_eq!(
            configured_update_endpoint(
                Some("  https://runtime.example.test/latest.json  "),
                Some("https://compile.example.test/latest.json"),
            ),
            "https://runtime.example.test/latest.json"
        );
        assert_eq!(
            configured_update_endpoint(
                Some("  "),
                Some("https://compile.example.test/latest.json"),
            ),
            "https://compile.example.test/latest.json"
        );
    }
}
