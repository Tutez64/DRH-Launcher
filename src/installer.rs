use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::ExtractedArchive;
use crate::diagnostics;
use crate::game_install::{game_executable_names, is_game_executable};
use crate::github_releases::PlatformRelease;
use crate::install_metadata::{InstalledRelease, InstalledState};
use crate::paths;
use crate::release_source::ReleaseSource;

fn log_install_step(install_dir: &Path, message: &str) {
    let _ = diagnostics::write(install_dir, diagnostics::LogLevel::Info, message);
}

#[cfg(test)]
pub fn install_extracted_archive(
    extracted: &ExtractedArchive,
    install_dir: &Path,
    release: &PlatformRelease,
    source: &ReleaseSource,
) -> Result<InstalledState, String> {
    install_extracted_archive_with_blocked_update(extracted, install_dir, release, source, None)
}

pub fn install_extracted_archive_with_blocked_update(
    extracted: &ExtractedArchive,
    install_dir: &Path,
    release: &PlatformRelease,
    source: &ReleaseSource,
    blocked_update_version: Option<String>,
) -> Result<InstalledState, String> {
    let source_game_dir = find_extracted_game_dir(&extracted.path)?;
    let game_dir = paths::game_dir(install_dir);
    let previous_game_dir = paths::previous_game_dir(install_dir);
    let retired_previous_dir = paths::game_root_dir(install_dir).join(".previous-replaced");
    if let Some(parent) = game_dir.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create game root directory {}: {error}",
                parent.display()
            )
        })?;
    }

    if retired_previous_dir.exists() {
        return Err(format!(
            "Temporary previous install directory already exists: {}",
            retired_previous_dir.display()
        ));
    }

    let previous_metadata = InstalledState::load(install_dir)
        .ok()
        .map(|state| state.active);

    if previous_game_dir.exists() {
        log_install_step(
            install_dir,
            &format!(
                "Removing existing previous install at {}.",
                previous_game_dir.display()
            ),
        );
        fs::rename(&previous_game_dir, &retired_previous_dir).map_err(|error| {
            format!(
                "Could not move {} to {}: {error}",
                previous_game_dir.display(),
                retired_previous_dir.display()
            )
        })?;
    }

    if game_dir.exists() {
        log_install_step(
            install_dir,
            &format!(
                "Moving current install {} to {}.",
                game_dir.display(),
                previous_game_dir.display()
            ),
        );
        fs::rename(&game_dir, &previous_game_dir).map_err(|error| {
            if retired_previous_dir.exists() {
                let _ = fs::rename(&retired_previous_dir, &previous_game_dir);
            }
            format!(
                "Could not move {} to {}: {error}",
                game_dir.display(),
                previous_game_dir.display()
            )
        })?;
    }

    log_install_step(
        install_dir,
        &format!(
            "Installing extracted game from {} to {}.",
            source_game_dir.display(),
            game_dir.display()
        ),
    );
    if let Err(error) = fs::rename(&source_game_dir, &game_dir) {
        if previous_game_dir.exists() {
            let _ = fs::rename(&previous_game_dir, &game_dir);
        }
        if retired_previous_dir.exists() {
            let _ = fs::rename(&retired_previous_dir, &previous_game_dir);
        }
        return Err(format!(
            "Could not move {} to {}: {error}",
            source_game_dir.display(),
            game_dir.display()
        ));
    }

    let installed = InstalledState {
        active: InstalledRelease::from_platform_release(release, source),
        previous: previous_metadata,
        blocked_update_version,
    };
    if let Err(error) = installed.save(install_dir) {
        let _ = fs::remove_dir_all(&game_dir);
        if previous_game_dir.exists() {
            let _ = fs::rename(&previous_game_dir, &game_dir);
        }
        if retired_previous_dir.exists() {
            let _ = fs::rename(&retired_previous_dir, &previous_game_dir);
        }
        return Err(format!("Could not write installed metadata: {error}"));
    }
    cleanup_replaced_dir(install_dir, &retired_previous_dir, "replaced previous install");
    log_install_step(
        install_dir,
        &format!(
            "Wrote installed metadata for {} (previous: {}).",
            installed.active.version,
            installed
                .previous
                .as_ref()
                .map(|previous| previous.version.as_str())
                .unwrap_or("none")
        ),
    );

    Ok(installed)
}

pub fn install_extracted_archive_preserving_previous(
    extracted: &ExtractedArchive,
    install_dir: &Path,
    release: &PlatformRelease,
    source: &ReleaseSource,
    blocked_update_version: Option<String>,
) -> Result<InstalledState, String> {
    let source_game_dir = find_extracted_game_dir(&extracted.path)?;
    let game_dir = paths::game_dir(install_dir);
    let backup_game_dir = paths::game_root_dir(install_dir).join(".preserve-current-backup");
    let existing = InstalledState::load(install_dir).ok();
    let previous_metadata = existing.as_ref().and_then(|state| state.previous.clone());
    let blocked_update_version = blocked_update_version.or_else(|| {
        existing
            .as_ref()
            .and_then(|state| state.blocked_update_version.clone())
    });

    if let Some(parent) = game_dir.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create game root directory {}: {error}",
                parent.display()
            )
        })?;
    }

    if backup_game_dir.exists() {
        return Err(format!(
            "Temporary current install directory already exists: {}",
            backup_game_dir.display()
        ));
    }

    if game_dir.exists() {
        log_install_step(
            install_dir,
            &format!(
                "Removing current install at {} (keeping previous).",
                game_dir.display()
            ),
        );
        fs::rename(&game_dir, &backup_game_dir).map_err(|error| {
            format!(
                "Could not move {} to {}: {error}",
                game_dir.display(),
                backup_game_dir.display()
            )
        })?;
    }

    log_install_step(
        install_dir,
        &format!(
            "Installing extracted game from {} to {}.",
            source_game_dir.display(),
            game_dir.display()
        ),
    );
    if let Err(error) = fs::rename(&source_game_dir, &game_dir) {
        if backup_game_dir.exists() {
            let _ = fs::rename(&backup_game_dir, &game_dir);
        }
        return Err(format!(
            "Could not move {} to {}: {error}",
            source_game_dir.display(),
            game_dir.display()
        ));
    }

    let installed = InstalledState {
        active: InstalledRelease::from_platform_release(release, source),
        previous: previous_metadata,
        blocked_update_version,
    };
    if let Err(error) = installed.save(install_dir) {
        let _ = fs::remove_dir_all(&game_dir);
        if backup_game_dir.exists() {
            let _ = fs::rename(&backup_game_dir, &game_dir);
        }
        return Err(format!("Could not write installed metadata: {error}"));
    }
    cleanup_replaced_dir(install_dir, &backup_game_dir, "replaced current install");
    log_install_step(
        install_dir,
        &format!(
            "Wrote installed metadata for {} (previous kept: {}).",
            installed.active.version,
            installed
                .previous
                .as_ref()
                .map(|previous| previous.version.as_str())
                .unwrap_or("none")
        ),
    );

    Ok(installed)
}

pub fn restore_previous_version(install_dir: &Path) -> Result<InstalledState, String> {
    let state = InstalledState::load(install_dir)?;
    let previous_metadata = state
        .previous
        .clone()
        .ok_or_else(|| "No previous version metadata is available.".to_string())?;
    let blocked_update_version = state.active.version.clone();
    let game_dir = paths::game_dir(install_dir);
    let previous_game_dir = paths::previous_game_dir(install_dir);
    let swap_dir = paths::game_root_dir(install_dir).join(".restore-swap");

    log_install_step(
        install_dir,
        &format!(
            "Restoring previous version {} (replacing {}).",
            previous_metadata.version, blocked_update_version
        ),
    );

    if swap_dir.exists() {
        return Err(format!(
            "Temporary restore directory already exists: {}",
            swap_dir.display()
        ));
    }

    let had_current_game_dir = game_dir.exists();
    if had_current_game_dir {
        fs::rename(&game_dir, &swap_dir).map_err(|error| {
            format!(
                "Could not move {} to {}: {error}",
                game_dir.display(),
                swap_dir.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(&previous_game_dir, &game_dir) {
        if had_current_game_dir {
            let _ = fs::rename(&swap_dir, &game_dir);
        }
        return Err(format!(
            "Could not move {} to {}: {error}",
            previous_game_dir.display(),
            game_dir.display()
        ));
    }

    let previous = if had_current_game_dir {
        match fs::rename(&swap_dir, &previous_game_dir) {
            Ok(()) => Some(state.active),
            Err(error) => {
                return Err(format!(
                    "Restored previous version, but could not preserve old current version: {error}"
                ));
            }
        }
    } else {
        None
    };

    let restored = InstalledState {
        active: previous_metadata,
        previous,
        blocked_update_version: Some(blocked_update_version),
    };
    if let Err(error) = restored.save(install_dir) {
        let _ = fs::rename(&previous_game_dir, &swap_dir);
        let _ = fs::rename(&game_dir, &previous_game_dir);
        if had_current_game_dir {
            let _ = fs::rename(&swap_dir, &game_dir);
        }
        return Err(format!("Could not write installed metadata: {error}"));
    }

    Ok(restored)
}

pub fn repair_active_install_from_extracted_archive(
    extracted: &ExtractedArchive,
    install_dir: &Path,
) -> Result<InstalledState, String> {
    let state = InstalledState::load(install_dir)?;
    repair_extracted_archive_with_state(extracted, install_dir, state)
}

pub fn repair_release_from_extracted_archive(
    extracted: &ExtractedArchive,
    install_dir: &Path,
    release: &PlatformRelease,
    source: &ReleaseSource,
) -> Result<InstalledState, String> {
    let previous = InstalledState::load(install_dir)
        .ok()
        .and_then(|state| state.previous);
    let state = InstalledState {
        active: InstalledRelease::from_platform_release(release, source),
        previous,
        blocked_update_version: None,
    };
    repair_extracted_archive_with_state(extracted, install_dir, state)
}

fn repair_extracted_archive_with_state(
    extracted: &ExtractedArchive,
    install_dir: &Path,
    state: InstalledState,
) -> Result<InstalledState, String> {
    let source_game_dir = find_extracted_game_dir(&extracted.path)?;
    let game_dir = paths::game_dir(install_dir);
    let backup_game_dir = paths::game_root_dir(install_dir).join(".repair-current-backup");
    if let Some(parent) = game_dir.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create game root directory {}: {error}",
                parent.display()
            )
        })?;
    }

    if backup_game_dir.exists() {
        return Err(format!(
            "Temporary repair directory already exists: {}",
            backup_game_dir.display()
        ));
    }

    if game_dir.exists() {
        fs::rename(&game_dir, &backup_game_dir).map_err(|error| {
            format!(
                "Could not move {} to {}: {error}",
                game_dir.display(),
                backup_game_dir.display()
            )
        })?;
    }

    if let Err(error) = fs::rename(&source_game_dir, &game_dir) {
        if backup_game_dir.exists() {
            let _ = fs::rename(&backup_game_dir, &game_dir);
        }
        return Err(format!(
            "Could not move {} to {}: {error}",
            source_game_dir.display(),
            game_dir.display()
        ));
    }

    if let Err(error) = state.save(install_dir) {
        let _ = fs::remove_dir_all(&game_dir);
        if backup_game_dir.exists() {
            let _ = fs::rename(&backup_game_dir, &game_dir);
        }
        return Err(format!("Could not write installed metadata: {error}"));
    }
    cleanup_replaced_dir(install_dir, &backup_game_dir, "replaced current install");

    Ok(state)
}

fn cleanup_replaced_dir(install_dir: &Path, path: &Path, description: &str) {
    if !path.exists() {
        return;
    }

    if let Err(error) = fs::remove_dir_all(path) {
        log_install_step(
            install_dir,
            &format!(
                "Could not remove {description} at {} after successful replacement: {error}",
                path.display()
            ),
        );
    }
}

pub fn find_extracted_game_dir(extracted_dir: &Path) -> Result<PathBuf, String> {
    if is_game_dir(extracted_dir) {
        return Ok(extracted_dir.to_path_buf());
    }

    let direct_game_dir = extracted_dir.join("game");
    if is_game_dir(&direct_game_dir) {
        return Ok(direct_game_dir);
    }

    let entries = fs::read_dir(extracted_dir)
        .map_err(|error| format!("Could not read {}: {error}", extracted_dir.display()))?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("Could not read extracted entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() && is_game_dir(&path) {
            candidates.push(path);
        }
    }

    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(format!(
            "Could not find extracted game directory containing one of: {}",
            game_executable_names().join(", ")
        )),
        _ => Err("Multiple extracted game directory candidates found".to_string()),
    }
}

fn is_game_dir(path: &Path) -> bool {
    game_executable_names()
        .iter()
        .any(|name| is_game_executable(&path.join(name)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github_releases::{ReleaseAsset, ReleaseMetadataSource};
    use tempfile::tempdir;

    #[test]
    fn finds_game_dir_at_extracted_root() {
        let temp = tempdir().unwrap();
        create_game_executable(&temp.path().join(primary_game_executable_name()), "");

        assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), temp.path());
    }

    #[test]
    fn finds_game_dir_under_game_folder() {
        let temp = tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        create_game_executable(&game.join(primary_game_executable_name()), "");

        assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), game);
    }

    #[test]
    fn installs_extracted_archive_and_preserves_previous_metadata() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        fs::create_dir_all(&install_dir).unwrap();
        let current_game = paths::game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        create_game_executable(&current_game.join(primary_game_executable_name()), "old");
        let previous = InstalledState {
            active: test_installed_release("V7"),
            previous: None,
            blocked_update_version: None,
        };
        previous.save(&install_dir).unwrap();

        let extracted_root = temp.path().join("extracted");
        let extracted_game = extracted_root.join("game");
        fs::create_dir_all(&extracted_game).unwrap();
        create_game_executable(&extracted_game.join(primary_game_executable_name()), "new");
        let extracted = ExtractedArchive {
            path: extracted_root,
        };

        let installed = install_extracted_archive(
            &extracted,
            &install_dir,
            &test_release("V9"),
            &ReleaseSource::fixtures(),
        )
        .unwrap();

        assert_eq!(installed.active.version, "V9");
        assert_eq!(installed.previous.unwrap().version, "V7");
        assert!(
            paths::game_dir(&install_dir)
                .join(primary_game_executable_name())
                .exists()
        );
        assert!(
            paths::previous_game_dir(&install_dir)
                .join(primary_game_executable_name())
                .exists()
        );
    }

    #[test]
    fn installs_extracted_archive_without_replacing_previous_slot() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        let current_game = paths::game_dir(&install_dir);
        let previous_game = paths::previous_game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::create_dir_all(&previous_game).unwrap();
        create_game_executable(
            &current_game.join(primary_game_executable_name()),
            "current",
        );
        create_game_executable(
            &previous_game.join(primary_game_executable_name()),
            "previous",
        );
        InstalledState {
            active: test_installed_release("V8"),
            previous: Some(test_installed_release("V10")),
            blocked_update_version: Some("V9".to_string()),
        }
        .save(&install_dir)
        .unwrap();

        let extracted_root = temp.path().join("extracted");
        let extracted_game = extracted_root.join("game");
        fs::create_dir_all(&extracted_game).unwrap();
        create_game_executable(&extracted_game.join(primary_game_executable_name()), "new");
        let extracted = ExtractedArchive {
            path: extracted_root,
        };

        let installed = install_extracted_archive_preserving_previous(
            &extracted,
            &install_dir,
            &test_release("V9"),
            &ReleaseSource::fixtures(),
            None,
        )
        .unwrap();

        assert_eq!(installed.active.version, "V9");
        assert_eq!(installed.previous.unwrap().version, "V10");
        assert_eq!(installed.blocked_update_version.as_deref(), Some("V9"));
        assert_eq!(
            read_game_executable(&current_game.join(primary_game_executable_name())),
            "new"
        );
        assert_eq!(
            read_game_executable(&previous_game.join(primary_game_executable_name())),
            "previous"
        );
    }

    #[test]
    fn restores_previous_version_and_keeps_old_current_as_previous() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        let current_game = paths::game_dir(&install_dir);
        let previous_game = paths::previous_game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::create_dir_all(&previous_game).unwrap();
        create_game_executable(
            &current_game.join(primary_game_executable_name()),
            "current",
        );
        create_game_executable(
            &previous_game.join(primary_game_executable_name()),
            "previous",
        );
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V7")),
            blocked_update_version: None,
        }
        .save(&install_dir)
        .unwrap();

        let restored = restore_previous_version(&install_dir).unwrap();

        assert_eq!(restored.active.version, "V7");
        assert_eq!(restored.previous.unwrap().version, "V9");
        assert_eq!(restored.blocked_update_version.as_deref(), Some("V9"));
        assert_eq!(
            read_game_executable(&current_game.join(primary_game_executable_name())),
            "previous"
        );
        assert_eq!(
            read_game_executable(&previous_game.join(primary_game_executable_name())),
            "current"
        );
        let metadata = InstalledState::load(&install_dir).unwrap();
        assert_eq!(metadata.active.version, "V7");
        assert_eq!(metadata.previous.unwrap().version, "V9");
        assert_eq!(metadata.blocked_update_version.as_deref(), Some("V9"));
    }

    #[test]
    fn restores_previous_version_even_when_it_is_not_launchable() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        let current_game = paths::game_dir(&install_dir);
        let previous_game = paths::previous_game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::create_dir_all(&previous_game).unwrap();
        create_game_executable(
            &current_game.join(primary_game_executable_name()),
            "current",
        );
        fs::write(previous_game.join("broken.txt"), "broken").unwrap();
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V7")),
            blocked_update_version: None,
        }
        .save(&install_dir)
        .unwrap();

        let restored = restore_previous_version(&install_dir).unwrap();

        assert_eq!(restored.active.version, "V7");
        assert_eq!(restored.previous.unwrap().version, "V9");
        assert!(current_game.join("broken.txt").exists());
        assert!(previous_game.join(primary_game_executable_name()).exists());
    }

    #[test]
    fn repairs_active_install_without_replacing_previous_version() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        let current_game = paths::game_dir(&install_dir);
        let previous_game = paths::previous_game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::create_dir_all(&previous_game).unwrap();
        fs::write(current_game.join("broken.txt"), "broken").unwrap();
        create_game_executable(
            &previous_game.join(primary_game_executable_name()),
            "previous",
        );
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V7")),
            blocked_update_version: Some("V10".to_string()),
        }
        .save(&install_dir)
        .unwrap();

        let extracted_root = temp.path().join("extracted");
        let extracted_game = extracted_root.join("game");
        fs::create_dir_all(&extracted_game).unwrap();
        create_game_executable(
            &extracted_game.join(primary_game_executable_name()),
            "repaired",
        );
        let extracted = ExtractedArchive {
            path: extracted_root,
        };

        let repaired = repair_active_install_from_extracted_archive(&extracted, &install_dir)
            .expect("repair should succeed");

        assert_eq!(repaired.active.version, "V9");
        assert_eq!(repaired.previous.unwrap().version, "V7");
        assert_eq!(repaired.blocked_update_version.as_deref(), Some("V10"));
        assert_eq!(
            read_game_executable(&current_game.join(primary_game_executable_name())),
            "repaired"
        );
        assert_eq!(
            read_game_executable(&previous_game.join(primary_game_executable_name())),
            "previous"
        );
    }

    #[test]
    fn repairs_with_release_without_replacing_previous_version() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        let current_game = paths::game_dir(&install_dir);
        let previous_game = paths::previous_game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::create_dir_all(&previous_game).unwrap();
        fs::write(current_game.join("broken.txt"), "broken").unwrap();
        create_game_executable(
            &previous_game.join(primary_game_executable_name()),
            "previous",
        );
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V7")),
            blocked_update_version: Some("V10".to_string()),
        }
        .save(&install_dir)
        .unwrap();

        let extracted_root = temp.path().join("extracted-release");
        let extracted_game = extracted_root.join("game");
        fs::create_dir_all(&extracted_game).unwrap();
        create_game_executable(
            &extracted_game.join(primary_game_executable_name()),
            "release",
        );
        let extracted = ExtractedArchive {
            path: extracted_root,
        };

        let repaired = repair_release_from_extracted_archive(
            &extracted,
            &install_dir,
            &test_release("V11"),
            &ReleaseSource::fixtures(),
        )
        .expect("repair should succeed");

        assert_eq!(repaired.active.version, "V11");
        assert_eq!(repaired.previous.unwrap().version, "V7");
        assert!(repaired.blocked_update_version.is_none());
        assert_eq!(
            read_game_executable(&current_game.join(primary_game_executable_name())),
            "release"
        );
        assert_eq!(
            read_game_executable(&previous_game.join(primary_game_executable_name())),
            "previous"
        );
    }

    #[test]
    fn does_not_treat_directory_named_like_executable_as_game_dir() {
        let temp = tempdir().unwrap();
        let archive_root = temp.path().join(primary_game_executable_name());
        fs::create_dir(&archive_root).unwrap();
        create_game_executable(&archive_root.join(primary_game_executable_name()), "");

        if cfg!(target_os = "macos") {
            assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), temp.path());
        } else {
            assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), archive_root);
        }
    }

    #[test]
    fn accepts_legacy_executable_name() {
        let temp = tempdir().unwrap();
        create_game_executable(&temp.path().join(legacy_primary_game_executable_name()), "");

        assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), temp.path());
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

    fn legacy_primary_game_executable_name() -> &'static str {
        game_executable_names()[1]
    }

    fn primary_game_executable_name() -> &'static str {
        game_executable_names()[0]
    }

    fn create_game_executable(path: &Path, contents: &str) {
        if cfg!(target_os = "macos") {
            let executable_path = macos_bundle_executable_path(path);
            fs::create_dir_all(executable_path.parent().unwrap()).unwrap();
            fs::write(executable_path, contents).unwrap();
        } else {
            fs::write(path, contents).unwrap();
        }
    }

    fn read_game_executable(path: &Path) -> String {
        if cfg!(target_os = "macos") {
            fs::read_to_string(macos_bundle_executable_path(path)).unwrap()
        } else {
            fs::read_to_string(path).unwrap()
        }
    }

    fn macos_bundle_executable_path(bundle_path: &Path) -> PathBuf {
        bundle_path
            .join("Contents")
            .join("MacOS")
            .join(bundle_path.file_stem().unwrap())
    }
}
