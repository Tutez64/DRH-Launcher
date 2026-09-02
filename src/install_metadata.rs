use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

use crate::atomic_file;
use crate::diagnostics;
use crate::github_releases::PlatformRelease;
use crate::paths;
use crate::release_manifest::{
    ManifestLaunchOptions, ReleaseManifest, manifest_includes_frame_rate,
    manifest_includes_steam_buildid, normalize_sha256,
};
use crate::release_source::ReleaseSource;
use crate::steam_buildid::resolve_steam_buildid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstalledState {
    pub active: InstalledRelease,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<InstalledRelease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_update_version: Option<String>,
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
        atomic_file::write(&path, contents.as_bytes())
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
    pub archive_size: u64,
    pub installed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub launch_options: Option<ManifestLaunchOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_buildid: Option<u64>,
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
            archive_size: release.asset.size,
            installed_at: current_timestamp(),
            launch_options: release.launch_options.clone(),
            steam_buildid: resolve_steam_buildid(&release.version, release.steam_buildid),
        }
    }

    pub fn apply_known_steam_buildid(&mut self) -> bool {
        if self.steam_buildid.is_some() {
            return false;
        }
        if let Some(buildid) = resolve_steam_buildid(&self.version, None) {
            self.steam_buildid = Some(buildid);
            true
        } else {
            false
        }
    }

    pub fn needs_steam_buildid_fetch(&self) -> bool {
        self.steam_buildid.is_none() && manifest_includes_steam_buildid(&self.version)
    }

    pub fn needs_frame_rate_fetch(&self) -> bool {
        self.launch_options
            .as_ref()
            .and_then(|options| options.frame_rate.as_ref())
            .is_none()
            && manifest_includes_frame_rate(&self.version)
    }

    pub fn needs_manifest_fetch(&self) -> bool {
        self.needs_steam_buildid_fetch() || self.needs_frame_rate_fetch()
    }

    pub fn apply_fetched_manifest(&mut self, manifest: &ReleaseManifest) -> bool {
        let mut changed = false;
        if self.needs_steam_buildid_fetch()
            && let Some(buildid) = resolve_steam_buildid(&self.version, manifest.steam_buildid)
        {
            self.steam_buildid = Some(buildid);
            changed = true;
        }
        if self.needs_frame_rate_fetch() {
            match (&mut self.launch_options, &manifest.launch_options) {
                (Some(existing), Some(fetched)) if existing.frame_rate.is_none() => {
                    existing.frame_rate = fetched.frame_rate.clone();
                    changed = true;
                }
                (None, Some(fetched)) => {
                    self.launch_options = Some(fetched.clone());
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }
}

fn current_timestamp() -> String {
    diagnostics::format_system_time_utc(std::time::SystemTime::now())
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
            blocked_update_version: Some("V3".to_string()),
        };

        state.save(temp.path()).unwrap();
        let loaded = InstalledState::load(temp.path()).unwrap();

        assert_eq!(loaded.active.version, "V2");
        assert_eq!(loaded.previous.unwrap().version, "V1");
        assert_eq!(loaded.blocked_update_version.as_deref(), Some("V3"));
    }

    #[test]
    fn builds_metadata_from_platform_release() {
        let release = PlatformRelease {
            version: "V9".to_string(),
            name: "Dungeon Rampage Haxe V9".to_string(),
            html_url: "https://example.test/V9".to_string(),
            metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
            launch_options: None,
            steam_buildid: Some(23435799),
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
        assert_eq!(metadata.archive_size, 123);
        assert!(metadata.launch_options.is_none());
        assert_eq!(metadata.steam_buildid, Some(23435799));
    }

    #[test]
    fn fills_catalog_steam_buildid_on_existing_install_metadata() {
        let mut metadata = test_installed_release("V13");

        assert!(metadata.apply_known_steam_buildid());
        assert_eq!(metadata.steam_buildid, Some(25038329));
        assert!(!metadata.apply_known_steam_buildid());
        assert!(!metadata.needs_steam_buildid_fetch());
    }

    #[test]
    fn only_fetches_manifest_fields_the_release_is_known_to_have() {
        let v10 = test_installed_release("V10");
        assert!(!v10.needs_frame_rate_fetch());
        assert!(!v10.needs_steam_buildid_fetch());
        assert!(!v10.needs_manifest_fetch());

        let v11 = test_installed_release("V11");
        assert!(v11.needs_frame_rate_fetch());
        assert!(!v11.needs_steam_buildid_fetch());
        assert!(v11.needs_manifest_fetch());

        let v13 = test_installed_release("V13");
        assert!(v13.needs_frame_rate_fetch());
        assert!(!v13.needs_steam_buildid_fetch());

        let v14 = test_installed_release("V14");
        assert!(v14.needs_frame_rate_fetch());
        assert!(v14.needs_steam_buildid_fetch());
        assert!(v14.needs_manifest_fetch());
    }

    #[test]
    fn fetched_manifest_fills_only_supported_missing_fields() {
        let mut release = test_installed_release("V14");
        let manifest = ReleaseManifest {
            version: "V14".to_string(),
            steam_buildid: Some(26_000_000),
            platforms: std::collections::HashMap::new(),
            launch_options: Some(ManifestLaunchOptions {
                frame_rate: Some(crate::release_manifest::ManifestFrameRate {
                    flag: "--fps".to_string(),
                    auto: crate::release_manifest::ManifestAutoFrameRate {
                        fallback: 120,
                        step: 24,
                        maximum: 240,
                    },
                    custom_min: 1,
                    custom_max: 10000,
                }),
                game_arguments: Vec::new(),
            }),
        };

        assert!(release.apply_fetched_manifest(&manifest));
        assert_eq!(release.steam_buildid, Some(26_000_000));
        assert!(
            release
                .launch_options
                .as_ref()
                .and_then(|options| options.frame_rate.as_ref())
                .is_some()
        );
        assert!(!release.needs_manifest_fetch());
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
            steam_buildid: None,
        }
    }
}
