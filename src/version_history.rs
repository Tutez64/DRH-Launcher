use std::sync::{Arc, Mutex};
use std::thread;

use crate::changelog_markdown::markdown_blocks;
use crate::config::LauncherConfig;
use crate::github_releases::{
    PlatformRelease, PlatformReleaseHistoryEntry, RepositoryRelease,
    discover_latest_platform_release, discover_platform_release_history,
    discover_repository_release_history,
};
use crate::home_view::{installed_active_release_version, restore_previous_release_version};
use crate::install_state::InstallState;
use crate::platform::Platform;
use crate::release_source::ReleaseSource;
use crate::{AppWindow, VersionEntryView, diagnostics, format_bytes, log_for_config};
use slint::{ComponentHandle, ModelRc, VecModel};

pub(crate) fn start_version_history_refresh(
    ui: &AppWindow,
    config: LauncherConfig,
    release_source: ReleaseSource,
    drh_version_history: Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: Arc<Mutex<Vec<RepositoryRelease>>>,
    pending_drh_version_install: Arc<Mutex<Option<String>>>,
) {
    log_for_config(
        &config,
        diagnostics::LogLevel::Info,
        "Checking release history.",
    );
    ui.set_refresh_version_history_enabled(false);
    ui.set_version_history_loading(true);
    ui.set_version_history_status("".into());
    ui.set_selected_version_action_enabled(false);

    let ui = ui.as_weak();
    thread::spawn(move || {
        let platform = Platform::current();
        let drh_result = discover_platform_release_history(&release_source, platform);
        let launcher_source = ReleaseSource::launcher();
        let launcher_result = discover_repository_release_history(&launcher_source);

        let log_status = match (&drh_result, &launcher_result) {
            (Ok(drh), Ok(launcher)) => {
                format!(
                    "Loaded {} DRH release(s) and {} DRH Launcher release(s).",
                    drh.len(),
                    launcher.len()
                )
            }
            (Err(drh_error), Ok(launcher)) => {
                format!(
                    "Loaded {} DRH Launcher release(s), but DRH history failed: {drh_error}",
                    launcher.len()
                )
            }
            (Ok(drh), Err(launcher_error)) => {
                format!(
                    "Loaded {} DRH release(s), but DRH Launcher history failed: {launcher_error}",
                    drh.len()
                )
            }
            (Err(drh_error), Err(launcher_error)) => {
                format!(
                    "Could not load release history. DRH: {drh_error}. DRH Launcher: {launcher_error}"
                )
            }
        };
        let status = match (&drh_result, &launcher_result) {
            (Ok(_), Ok(_)) => String::new(),
            (Err(drh_error), Ok(_)) => {
                format!("Could not load DRH releases: {drh_error}")
            }
            (Ok(_), Err(launcher_error)) => {
                format!("Could not load DRH Launcher releases: {launcher_error}")
            }
            (Err(drh_error), Err(launcher_error)) => {
                format!(
                    "Could not load release history. DRH: {drh_error}. DRH Launcher: {launcher_error}"
                )
            }
        };
        let level = if drh_result.is_ok() || launcher_result.is_ok() {
            diagnostics::LogLevel::Info
        } else {
            diagnostics::LogLevel::Error
        };
        log_for_config(&config, level, &log_status);

        let drh_entries = drh_result.ok();
        let launcher_entries = launcher_result.ok();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = ui.upgrade() else {
                return;
            };

            if let Some(entries) = drh_entries {
                drh_version_history
                    .lock()
                    .expect("DRH version history lock poisoned")
                    .clone_from(&entries);
            }
            if let Some(entries) = launcher_entries {
                launcher_version_history
                    .lock()
                    .expect("launcher version history lock poisoned")
                    .clone_from(&entries);
            }
            pending_drh_version_install
                .lock()
                .expect("pending DRH version install lock poisoned")
                .take();
            ui.set_version_history_status(status.into());
            ui.set_version_history_loading(false);
            ui.set_refresh_version_history_enabled(true);
            refresh_version_history_selection(
                &ui,
                &config,
                &drh_version_history,
                &launcher_version_history,
                &pending_drh_version_install,
            );
        });
    });
}

pub(crate) fn refresh_version_history_selection(
    ui: &AppWindow,
    config: &LauncherConfig,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: &Arc<Mutex<Vec<RepositoryRelease>>>,
    pending_drh_version_install: &Arc<Mutex<Option<String>>>,
) {
    let drh_entries = drh_version_history
        .lock()
        .expect("DRH version history lock poisoned")
        .clone();
    let launcher_entries = launcher_version_history
        .lock()
        .expect("launcher version history lock poisoned")
        .clone();
    let pending_version = pending_drh_version_install
        .lock()
        .expect("pending DRH version install lock poisoned")
        .clone();

    let drh_views = drh_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| drh_version_entry_view(config, entry, index))
        .collect::<Vec<_>>();
    ui.set_drh_version_list_label(release_list_label("DRH releases", drh_views.len()).into());
    ui.set_drh_version_entries(ModelRc::new(VecModel::from(drh_views)));

    let launcher_views = launcher_entries
        .iter()
        .enumerate()
        .map(|(index, release)| launcher_version_entry_view(release, index))
        .collect::<Vec<_>>();
    ui.set_launcher_version_list_label(
        release_list_label("DRH Launcher releases", launcher_views.len()).into(),
    );
    ui.set_launcher_version_entries(ModelRc::new(VecModel::from(launcher_views)));

    apply_selected_version_view(
        ui,
        config,
        &drh_entries,
        &launcher_entries,
        pending_version.as_deref(),
    );
}

pub(crate) fn refresh_selected_version_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: &Arc<Mutex<Vec<RepositoryRelease>>>,
    pending_drh_version_install: &Arc<Mutex<Option<String>>>,
) {
    let drh_entries = drh_version_history
        .lock()
        .expect("DRH version history lock poisoned")
        .clone();
    let launcher_entries = launcher_version_history
        .lock()
        .expect("launcher version history lock poisoned")
        .clone();
    let pending_version = pending_drh_version_install
        .lock()
        .expect("pending DRH version install lock poisoned")
        .clone();

    apply_selected_version_view(
        ui,
        config,
        &drh_entries,
        &launcher_entries,
        pending_version.as_deref(),
    );
}

fn apply_selected_version_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    drh_entries: &[PlatformReleaseHistoryEntry],
    launcher_entries: &[RepositoryRelease],
    pending_version: Option<&str>,
) {
    if ui.get_version_history_page() == 0 {
        let Some(index) =
            normalize_selected_index(ui.get_selected_drh_version_index(), drh_entries.len())
        else {
            ui.set_selected_drh_version_index(-1);
            clear_selected_version_view(ui);
            return;
        };
        ui.set_selected_drh_version_index(index as i32);
        apply_selected_drh_version_view(ui, config, &drh_entries[index], pending_version, drh_entries);
    } else {
        let Some(index) = normalize_selected_index(
            ui.get_selected_launcher_version_index(),
            launcher_entries.len(),
        ) else {
            ui.set_selected_launcher_version_index(-1);
            clear_selected_version_view(ui);
            return;
        };
        ui.set_selected_launcher_version_index(index as i32);
        apply_selected_launcher_version_view(ui, &launcher_entries[index], index);
    }
}

fn normalize_selected_index(current: i32, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if current < 0 || current as usize >= len {
        Some(0)
    } else {
        Some(current as usize)
    }
}

fn release_list_label(label: &str, count: usize) -> String {
    format!("{label} ({count})")
}

fn clear_selected_version_view(ui: &AppWindow) {
    ui.set_selected_version_title("No release selected".into());
    ui.set_selected_version_detail("".into());
    ui.set_selected_version_changelog_blocks(ModelRc::new(VecModel::from(markdown_blocks(
        "Select a release to read its changelog.",
    ))));
    ui.set_selected_version_release_enabled(false);
    ui.set_selected_version_action_text("Install selected version".into());
    ui.set_selected_version_action_enabled(false);
    ui.set_selected_version_replace_confirmation_visible(false);
    ui.set_selected_version_replace_confirmation_text("".into());
}

fn drh_version_entry_view(
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    index: usize,
) -> VersionEntryView {
    VersionEntryView {
        title: release_title(&entry.release.version, &entry.release.name).into(),
        detail: drh_version_entry_detail(entry).into(),
        status: drh_version_status(config, entry, index).into(),
    }
}

fn launcher_version_entry_view(release: &RepositoryRelease, index: usize) -> VersionEntryView {
    VersionEntryView {
        title: release_title(&release.version, &release.name).into(),
        detail: repository_release_detail(release).into(),
        status: launcher_version_status(release, index).into(),
    }
}

fn apply_selected_drh_version_view(
    ui: &AppWindow,
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    pending_version: Option<&str>,
    drh_entries: &[PlatformReleaseHistoryEntry],
) {
    let (action_text, action_enabled, confirmation_visible) =
        selected_drh_version_action(config, entry, pending_version, version_action_blocker(ui));
    ui.set_selected_version_title(
        release_title(&entry.release.version, &entry.release.name).into(),
    );
    ui.set_selected_version_detail(selected_drh_version_detail(entry).into());
    ui.set_selected_version_changelog_blocks(ModelRc::new(VecModel::from(markdown_blocks(
        &entry.release.body,
    ))));
    ui.set_selected_version_release_enabled(true);
    ui.set_selected_version_action_text(action_text.into());
    ui.set_selected_version_action_enabled(action_enabled);
    ui.set_selected_version_replace_confirmation_visible(confirmation_visible);
    ui.set_selected_version_replace_confirmation_text(
        if confirmation_visible {
            selected_drh_version_replace_confirmation_text(config, drh_entries, entry).into()
        } else {
            "".into()
        },
    );
}

fn apply_selected_launcher_version_view(ui: &AppWindow, release: &RepositoryRelease, index: usize) {
    let mut detail = repository_release_detail(release);
    let status = launcher_version_status(release, index);
    if !status.is_empty() {
        detail = format!("{detail} · {status}");
    }

    ui.set_selected_version_title(release_title(&release.version, &release.name).into());
    ui.set_selected_version_detail(detail.into());
    ui.set_selected_version_changelog_blocks(ModelRc::new(VecModel::from(markdown_blocks(
        &release.body,
    ))));
    ui.set_selected_version_release_enabled(true);
    ui.set_selected_version_action_text("Use Home update banner".into());
    ui.set_selected_version_action_enabled(false);
    ui.set_selected_version_replace_confirmation_visible(false);
    ui.set_selected_version_replace_confirmation_text("".into());
}

pub(crate) fn selected_drh_history_entry(
    ui: &AppWindow,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
) -> Option<PlatformReleaseHistoryEntry> {
    let index = ui.get_selected_drh_version_index();
    if index < 0 {
        return None;
    }
    drh_version_history
        .lock()
        .expect("DRH version history lock poisoned")
        .get(index as usize)
        .cloned()
}

pub(crate) fn selected_history_blocked_update_version(
    latest_version: Option<&str>,
    selected_version: &str,
) -> Option<String> {
    latest_version
        .filter(|latest_version| latest_version.trim() != selected_version.trim())
        .map(ToOwned::to_owned)
}

pub(crate) fn latest_drh_release_version_for_update_block(
    latest_release: &Arc<Mutex<Option<PlatformRelease>>>,
    release_source: &ReleaseSource,
) -> Option<String> {
    let cached = latest_release
        .lock()
        .expect("latest release lock poisoned")
        .as_ref()
        .map(|release| release.version.clone());
    cached.or_else(|| {
        discover_latest_platform_release(release_source, Platform::current())
            .ok()
            .map(|release| release.version)
    })
}

pub(crate) fn selected_version_release_url(
    ui: &AppWindow,
    drh_version_history: &Arc<Mutex<Vec<PlatformReleaseHistoryEntry>>>,
    launcher_version_history: &Arc<Mutex<Vec<RepositoryRelease>>>,
) -> Option<String> {
    if ui.get_version_history_page() == 0 {
        selected_drh_history_entry(ui, drh_version_history).map(|entry| entry.release.html_url)
    } else {
        let index = ui.get_selected_launcher_version_index();
        if index < 0 {
            return None;
        }
        launcher_version_history
            .lock()
            .expect("launcher version history lock poisoned")
            .get(index as usize)
            .map(|release| release.html_url.clone())
    }
}

fn drh_version_entry_detail(entry: &PlatformReleaseHistoryEntry) -> String {
    let mut parts = vec![release_date(&entry.release)];
    if entry.release.prerelease {
        parts.push("pre-release".to_string());
    }
    match &entry.platform_release {
        Some(release) => {
            parts.push(format_bytes(release.asset.size));
            parts.push(release.metadata_source.label().to_string());
        }
        None if entry.manifest_available => {
            parts.push("manifest metadata".to_string());
        }
        None => parts.push(format!("no {} package", Platform::current().id())),
    }
    parts.join(" · ")
}

fn selected_drh_version_detail(entry: &PlatformReleaseHistoryEntry) -> String {
    let mut parts = vec![format!("Published: {}", release_date(&entry.release))];
    if entry.release.prerelease {
        parts.push("Pre-release".to_string());
    }
    match &entry.platform_release {
        Some(release) => {
            parts.push(format!("Asset: {}", release.asset.name));
            parts.push(format!("Size: {}", format_bytes(release.asset.size)));
            parts.push(format!("Metadata: {}", release.metadata_source.label()));
        }
        None if entry.manifest_available => {
            parts.push(
                "Package details will be resolved from the release manifest before install."
                    .to_string(),
            );
        }
        None => {
            parts.push(format!(
                "Unavailable on {}: {}",
                Platform::current().id(),
                entry
                    .unsupported_reason
                    .as_deref()
                    .unwrap_or("no compatible package was found")
            ));
        }
    }
    parts.join("\n")
}

fn repository_release_detail(release: &RepositoryRelease) -> String {
    let mut parts = vec![release_date(release)];
    if release.prerelease {
        parts.push("pre-release".to_string());
    }
    parts.join(" · ")
}

fn drh_version_status(
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    index: usize,
) -> String {
    if installed_active_release_version(config)
        .as_deref()
        .is_some_and(|version| version.trim() == entry.release.version.trim())
        || restore_previous_release_version(config)
            .as_deref()
            .is_some_and(|version| version.trim() == entry.release.version.trim())
    {
        "Installed".to_string()
    } else if entry.platform_release.is_none() && !entry.manifest_available {
        "Unavailable".to_string()
    } else if entry.platform_release.is_none() && entry.manifest_available {
        "Manifest".to_string()
    } else if index == 0 {
        "Latest".to_string()
    } else if entry.release.prerelease {
        "Pre-release".to_string()
    } else {
        "Older".to_string()
    }
}

fn launcher_version_status(release: &RepositoryRelease, index: usize) -> String {
    if normalized_version_tag(&release.version) == normalized_version_tag(env!("CARGO_PKG_VERSION"))
    {
        "Current".to_string()
    } else if index == 0 {
        "Latest".to_string()
    } else if release.prerelease {
        "Pre-release".to_string()
    } else {
        "Older".to_string()
    }
}

fn selected_drh_version_action(
    config: &LauncherConfig,
    entry: &PlatformReleaseHistoryEntry,
    pending_version: Option<&str>,
    blocker: Option<&str>,
) -> (String, bool, bool) {
    if entry.platform_release.is_none() && !entry.manifest_available {
        return ("Unavailable".to_string(), false, false);
    }

    let version = entry.release.version.as_str();
    match installed_active_release_version(config) {
        Some(installed) if installed.trim() == version.trim() => {
            ("Installed".to_string(), false, false)
        }
        _ if restore_previous_release_version(config)
            .as_deref()
            .is_some_and(|previous| previous.trim() == version.trim()) =>
        {
            ("Use Restore".to_string(), false, false)
        }
        _ if let Some(blocker) = blocker => (blocker.to_string(), false, false),
        Some(_) if pending_version == Some(version) => (format!("Install {version}"), true, true),
        Some(_) => (format!("Install {version}"), true, false),
        None => (format!("Install {version}"), true, false),
    }
}

fn drh_history_index(entries: &[PlatformReleaseHistoryEntry], version: &str) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.release.version.trim() == version.trim())
}

pub(crate) fn preserve_previous_slot_on_install(
    entries: &[PlatformReleaseHistoryEntry],
    config: &LauncherConfig,
    target_version: &str,
) -> bool {
    let Some(active) = installed_active_release_version(config) else {
        return false;
    };
    let Some(previous) = restore_previous_release_version(config) else {
        return false;
    };
    preserve_previous_slot_on_install_versions(entries, &active, &previous, target_version)
}

pub(crate) fn preserve_previous_slot_on_install_versions(
    entries: &[PlatformReleaseHistoryEntry],
    active_version: &str,
    previous_version: &str,
    target_version: &str,
) -> bool {
    if target_version.trim() == active_version.trim()
        || target_version.trim() == previous_version.trim()
    {
        return false;
    }

    let Some(active_idx) = drh_history_index(entries, active_version) else {
        return false;
    };
    let Some(previous_idx) = drh_history_index(entries, previous_version) else {
        return false;
    };
    let Some(target_idx) = drh_history_index(entries, target_version) else {
        return false;
    };

    if previous_idx >= active_idx {
        return false;
    }

    target_idx > active_idx || (target_idx < active_idx && target_idx > previous_idx)
}

fn selected_drh_version_replace_confirmation_text(
    config: &LauncherConfig,
    entries: &[PlatformReleaseHistoryEntry],
    entry: &PlatformReleaseHistoryEntry,
) -> String {
    let target = entry.release.version.as_str();
    let active = installed_active_release_version(config).unwrap_or_else(|| "unknown".to_string());
    let previous = restore_previous_release_version(config);
    let preserve_previous = preserve_previous_slot_on_install(entries, config, target);

    let mut message = format!("Installing {target} will replace {active} (current).");

    if preserve_previous {
        if let Some(previous) = &previous {
            message.push_str(&format!(
                "\n{previous} will remain on disk as the Restore version."
            ));
        }
        message.push_str(&format!("\n{active} will be removed from disk."));
    } else {
        message.push_str(&format!(
            "\n{active} will remain on disk as the Restore version."
        ));
        if let Some(previous) = previous {
            message.push_str(&format!(
                "\n{previous} (Restore) will be removed from disk."
            ));
        }
    }

    message.push_str(
        "\n\nOlder releases may no longer connect properly, may miss features or may be unstable.",
    );
    message
}

fn version_action_blocker(ui: &AppWindow) -> Option<&'static str> {
    let install_action = ui.get_install_action_text();
    if install_action == InstallState::Updating.primary_action() {
        Some("Installing...")
    } else if install_action == InstallState::Playing.primary_action() {
        Some("Stop DRH first")
    } else {
        None
    }
}

fn release_title(version: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || name == version || name.contains(version) {
        if name.is_empty() {
            version.to_string()
        } else {
            name.to_string()
        }
    } else {
        format!("{version} - {name}")
    }
}

fn release_date(release: &RepositoryRelease) -> String {
    release
        .published_at
        .as_deref()
        .filter(|published_at| published_at.len() >= 10)
        .map(|published_at| published_at[..10].to_string())
        .or_else(|| release.published_at.clone())
        .unwrap_or_else(|| "unknown date".to_string())
}

fn normalized_version_tag(version: &str) -> &str {
    version.trim().trim_start_matches('v')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github_releases::{ReleaseAsset, ReleaseMetadataSource};
    use crate::install_metadata::{InstalledRelease, InstalledState};
    use tempfile::tempdir;

    #[test]
    fn historical_install_blocks_latest_loaded_release() {
        assert_eq!(
            selected_history_blocked_update_version(Some("V10"), "V7"),
            Some("V10".to_string())
        );
        assert_eq!(
            selected_history_blocked_update_version(Some("V10"), "V10"),
            None
        );
        assert_eq!(selected_history_blocked_update_version(None, "V7"), None);
    }

    #[test]
    fn marks_active_and_previous_history_versions_as_installed() {
        let temp = tempdir().unwrap();
        InstalledState {
            active: test_installed_release("V7"),
            previous: Some(test_installed_release("V10")),
            blocked_update_version: Some("V10".to_string()),
        }
        .save(temp.path())
        .unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };
        let active_entry = test_history_entry("V7");
        let previous_entry = test_history_entry("V10");

        assert_eq!(drh_version_status(&config, &active_entry, 1), "Installed");
        assert_eq!(drh_version_status(&config, &previous_entry, 0), "Installed");
        assert_eq!(
            selected_drh_version_action(&config, &active_entry, None, None),
            ("Installed".to_string(), false, false)
        );
        assert_eq!(
            selected_drh_version_action(&config, &previous_entry, None, None),
            ("Use Restore".to_string(), false, false)
        );
    }

    #[test]
    fn preserves_previous_slot_after_restore_when_installing_between_versions() {
        let entries = vec![
            test_history_entry("V10"),
            test_history_entry("V9"),
            test_history_entry("V8"),
            test_history_entry("V7"),
        ];

        assert!(preserve_previous_slot_on_install_versions(
            &entries, "V8", "V10", "V9"
        ));
        assert!(preserve_previous_slot_on_install_versions(
            &entries, "V8", "V10", "V7"
        ));
        assert!(!preserve_previous_slot_on_install_versions(
            &entries, "V10", "V9", "V8"
        ));
        assert!(!preserve_previous_slot_on_install_versions(
            &entries, "V8", "V10", "V10"
        ));
    }

    #[test]
    fn replace_confirmation_mentions_current_as_restore_fallback() {
        let temp = tempdir().unwrap();
        InstalledState {
            active: test_installed_release("V10"),
            previous: None,
            blocked_update_version: None,
        }
        .save(temp.path())
        .unwrap();
        let config = LauncherConfig {
            install_dir: Some(temp.path().to_path_buf()),
            ..LauncherConfig::default()
        };
        let entries = vec![test_history_entry("V10"), test_history_entry("V9")];
        let entry = test_history_entry("V9");

        let message = selected_drh_version_replace_confirmation_text(&config, &entries, &entry);

        assert!(message.contains("Installing V9 will replace V10 (current)."));
        assert!(message.contains("V10 will remain on disk as the Restore version."));
    }

    fn test_history_entry(version: &str) -> PlatformReleaseHistoryEntry {
        PlatformReleaseHistoryEntry {
            release: RepositoryRelease {
                version: version.to_string(),
                name: format!("Dungeon Rampage Haxe {version}"),
                html_url: format!("https://example.test/{version}"),
                body: format!("Changelog for {version}"),
                published_at: Some("2026-01-01T00:00:00Z".to_string()),
                prerelease: false,
            },
            platform_release: Some(test_release(version)),
            manifest_available: false,
            unsupported_reason: None,
        }
    }

    fn test_release(version: &str) -> PlatformRelease {
        PlatformRelease {
            version: version.to_string(),
            name: format!("Dungeon Rampage Haxe {version}"),
            html_url: format!("https://example.test/{version}"),
            metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
            launch_options: None,
            asset: ReleaseAsset {
                platform_id: "linux-x64".to_string(),
                name: format!("Dungeon.Rampage.Haxe.{version}.Linux.tar.gz"),
                download_url: "https://example.test/archive.tar.gz".to_string(),
                size: 123,
                digest: Some("sha256:abc123".to_string()),
            },
        }
    }

    fn test_installed_release(version: &str) -> InstalledRelease {
        InstalledRelease {
            version: version.to_string(),
            platform: "linux-x64".to_string(),
            source: "Tutez64/DRHL-Release-Fixtures".to_string(),
            release_url: format!("https://example.test/{version}"),
            archive: format!("archive-{version}.tar.gz"),
            archive_sha256: "abc123".to_string(),
            archive_size: 123,
            installed_at: "unix:0".to_string(),
            launch_options: None,
        }
    }
}
