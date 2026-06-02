use std::fs;
use std::path::{Path, PathBuf};

use crate::archive::ExtractedArchive;
use crate::game_install::{game_executable_names, is_game_executable};
use crate::github_releases::PlatformRelease;
use crate::install_metadata::{InstalledRelease, InstalledState};
use crate::paths;
use crate::release_source::ReleaseSource;

pub fn install_extracted_archive(
    extracted: &ExtractedArchive,
    install_dir: &Path,
    release: &PlatformRelease,
    source: &ReleaseSource,
) -> Result<InstalledState, String> {
    let source_game_dir = find_extracted_game_dir(&extracted.path)?;
    let game_dir = paths::game_dir(install_dir);
    let previous_game_dir = paths::previous_game_dir(install_dir);
    if let Some(parent) = game_dir.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create game root directory {}: {error}",
                parent.display()
            )
        })?;
    }

    let previous_metadata = InstalledState::load(install_dir)
        .ok()
        .map(|state| state.active);

    if previous_game_dir.exists() {
        fs::remove_dir_all(&previous_game_dir).map_err(|error| {
            format!(
                "Could not remove previous game directory {}: {error}",
                previous_game_dir.display()
            )
        })?;
    }

    if game_dir.exists() {
        fs::rename(&game_dir, &previous_game_dir).map_err(|error| {
            format!(
                "Could not move {} to {}: {error}",
                game_dir.display(),
                previous_game_dir.display()
            )
        })?;
    }

    fs::rename(&source_game_dir, &game_dir).map_err(|error| {
        if previous_game_dir.exists() {
            let _ = fs::rename(&previous_game_dir, &game_dir);
        }
        format!(
            "Could not move {} to {}: {error}",
            source_game_dir.display(),
            game_dir.display()
        )
    })?;

    let installed = InstalledState {
        active: InstalledRelease::from_platform_release(release, source),
        previous: previous_metadata,
        blocked_update_version: None,
    };
    installed
        .save(install_dir)
        .map_err(|error| format!("Could not write installed metadata: {error}"))?;

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

    if !is_game_dir(&previous_game_dir) {
        return Err(format!(
            "Previous game directory is not launchable: {}",
            previous_game_dir.display()
        ));
    }

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
    restored
        .save(install_dir)
        .map_err(|error| format!("Could not write installed metadata: {error}"))?;

    Ok(restored)
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
        fs::write(temp.path().join(primary_game_executable_name()), "").unwrap();

        assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), temp.path());
    }

    #[test]
    fn finds_game_dir_under_game_folder() {
        let temp = tempdir().unwrap();
        let game = temp.path().join("game");
        fs::create_dir(&game).unwrap();
        fs::write(game.join(primary_game_executable_name()), "").unwrap();

        assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), game);
    }

    #[test]
    fn installs_extracted_archive_and_preserves_previous_metadata() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        fs::create_dir_all(&install_dir).unwrap();
        let current_game = paths::game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::write(current_game.join(primary_game_executable_name()), "old").unwrap();
        let previous = InstalledState {
            active: test_installed_release("V7"),
            previous: None,
            blocked_update_version: None,
        };
        previous.save(&install_dir).unwrap();

        let extracted_root = temp.path().join("extracted");
        let extracted_game = extracted_root.join("game");
        fs::create_dir_all(&extracted_game).unwrap();
        fs::write(extracted_game.join(primary_game_executable_name()), "new").unwrap();
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
    fn restores_previous_version_and_keeps_old_current_as_previous() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        let current_game = paths::game_dir(&install_dir);
        let previous_game = paths::previous_game_dir(&install_dir);
        fs::create_dir_all(&current_game).unwrap();
        fs::create_dir_all(&previous_game).unwrap();
        fs::write(current_game.join(primary_game_executable_name()), "current").unwrap();
        fs::write(
            previous_game.join(primary_game_executable_name()),
            "previous",
        )
        .unwrap();
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
            fs::read_to_string(current_game.join(primary_game_executable_name())).unwrap(),
            "previous"
        );
        assert_eq!(
            fs::read_to_string(previous_game.join(primary_game_executable_name())).unwrap(),
            "current"
        );
        let metadata = InstalledState::load(&install_dir).unwrap();
        assert_eq!(metadata.active.version, "V7");
        assert_eq!(metadata.previous.unwrap().version, "V9");
        assert_eq!(metadata.blocked_update_version.as_deref(), Some("V9"));
    }

    #[test]
    fn refuses_restore_when_previous_game_is_not_launchable() {
        let temp = tempdir().unwrap();
        let install_dir = temp.path().join("install");
        fs::create_dir_all(paths::previous_game_dir(&install_dir)).unwrap();
        InstalledState {
            active: test_installed_release("V9"),
            previous: Some(test_installed_release("V7")),
            blocked_update_version: None,
        }
        .save(&install_dir)
        .unwrap();

        let error = restore_previous_version(&install_dir).unwrap_err();

        assert!(error.contains("Previous game directory is not launchable"));
    }

    #[test]
    fn does_not_treat_directory_named_like_executable_as_game_dir() {
        let temp = tempdir().unwrap();
        let archive_root = temp.path().join(primary_game_executable_name());
        fs::create_dir(&archive_root).unwrap();
        fs::write(archive_root.join(primary_game_executable_name()), "").unwrap();

        assert_eq!(find_extracted_game_dir(temp.path()).unwrap(), archive_root);
    }

    #[test]
    fn accepts_legacy_executable_name() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join(legacy_primary_game_executable_name()), "").unwrap();

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
}
