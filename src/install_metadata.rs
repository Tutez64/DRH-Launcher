use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::github_releases::PlatformRelease;
use crate::paths;
use crate::release_manifest::normalize_sha256;
use crate::release_source::ReleaseSource;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstalledState {
    pub active: InstalledRelease,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<InstalledRelease>,
}

impl InstalledState {
    pub fn load(install_dir: &Path) -> Result<Self, String> {
        let path = paths::installed_metadata_file(install_dir);
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        serde_json::from_str(&contents)
            .map_err(|error| format!("Could not parse {}: {error}", path.display()))
    }

    pub fn save(&self, install_dir: &Path) -> io::Result<()> {
        let path = paths::installed_metadata_file(install_dir);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::write(path, contents)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstalledRelease {
    pub version: String,
    pub platform: String,
    pub source: String,
    pub release_url: String,
    pub archive: String,
    pub archive_sha256: String,
    pub installed_at: String,
}

impl InstalledRelease {
    pub fn from_platform_release(release: &PlatformRelease, source: &ReleaseSource) -> Self {
        Self {
            version: release.version.clone(),
            platform: release.asset.platform_id.clone(),
            source: source.label(),
            release_url: release.html_url.clone(),
            archive: release.asset.name.clone(),
            archive_sha256: release
                .asset
                .digest
                .as_deref()
                .map(normalize_sha256)
                .unwrap_or_default(),
            installed_at: current_timestamp(),
        }
    }
}

fn current_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("unix:{seconds}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github_releases::{PlatformRelease, ReleaseAsset, ReleaseMetadataSource};
    use tempfile::tempdir;

    #[test]
    fn saves_and_loads_installed_state() {
        let temp = tempdir().unwrap();
        let state = InstalledState {
            active: test_installed_release("V2"),
            previous: Some(test_installed_release("V1")),
        };

        state.save(temp.path()).unwrap();
        let loaded = InstalledState::load(temp.path()).unwrap();

        assert_eq!(loaded.active.version, "V2");
        assert_eq!(loaded.previous.unwrap().version, "V1");
    }

    #[test]
    fn builds_metadata_from_platform_release() {
        let release = PlatformRelease {
            version: "V9".to_string(),
            name: "Dungeon Rampage Haxe V9".to_string(),
            html_url: "https://example.test/V9".to_string(),
            metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
            launch_options: None,
            asset: ReleaseAsset {
                platform_id: "linux-x64".to_string(),
                name: "Dungeon.Rampage.Haxe.V9.Linux.tar.gz".to_string(),
                download_url: "https://example.test/archive.tar.gz".to_string(),
                size: 123,
                digest: Some("sha256:abc123".to_string()),
            },
        };

        let metadata =
            InstalledRelease::from_platform_release(&release, &ReleaseSource::fixtures());

        assert_eq!(metadata.version, "V9");
        assert_eq!(metadata.platform, "linux-x64");
        assert_eq!(metadata.source, "Tutez64/DRHL-Release-Fixtures");
        assert_eq!(metadata.archive, "Dungeon.Rampage.Haxe.V9.Linux.tar.gz");
        assert_eq!(metadata.archive_sha256, "abc123");
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
        }
    }
}
