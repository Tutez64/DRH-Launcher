use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;
use std::time::Duration;

use crate::platform::Platform;
use crate::release_manifest::{
    ManifestLaunchOptions, ReleaseManifest, is_manifest_asset_name, normalize_sha256,
};
use crate::release_source::ReleaseSource;

#[derive(Clone, Debug)]
pub struct PlatformRelease {
    pub version: String,
    pub name: String,
    pub html_url: String,
    pub metadata_source: ReleaseMetadataSource,
    pub asset: ReleaseAsset,
    pub launch_options: Option<ManifestLaunchOptions>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseMetadataSource {
    Manifest,
    GitHubAssetFallback,
}

impl ReleaseMetadataSource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::GitHubAssetFallback => "GitHub asset fallback",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReleaseAsset {
    pub platform_id: String,
    pub name: String,
    pub download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

pub fn discover_latest_platform_release(
    source: &ReleaseSource,
    platform: Platform,
) -> Result<PlatformRelease, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not create GitHub client: {error}"))?;

    let release: GitHubRelease = client
        .get(source.api_latest_release_url())
        .header(USER_AGENT, "DRH-Launcher")
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|error| format!("Could not check GitHub releases: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub release check failed: {error}"))?
        .json()
        .map_err(|error| format!("Could not parse GitHub release response: {error}"))?;

    let manifest = fetch_manifest_if_available(&client, &release)?;
    select_platform_release(release, platform, manifest.as_ref())
}

fn fetch_manifest_if_available(
    client: &Client,
    release: &GitHubRelease,
) -> Result<Option<ReleaseManifest>, String> {
    let Some(manifest_asset) = release
        .assets
        .iter()
        .find(|asset| is_manifest_asset_name(&asset.name, &release.tag_name))
    else {
        return Ok(None);
    };

    let contents = client
        .get(&manifest_asset.browser_download_url)
        .header(USER_AGENT, "DRH-Launcher")
        .header(ACCEPT, "application/octet-stream")
        .send()
        .map_err(|error| format!("Could not download release manifest: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Release manifest download failed: {error}"))?
        .text()
        .map_err(|error| format!("Could not read release manifest: {error}"))?;

    ReleaseManifest::parse(&contents).map(Some)
}

fn select_platform_release(
    release: GitHubRelease,
    platform: Platform,
    manifest: Option<&ReleaseManifest>,
) -> Result<PlatformRelease, String> {
    if let Some(manifest) = manifest {
        return select_manifest_platform(release, platform, manifest);
    }

    select_platform_asset_fallback(release, platform)
}

fn select_manifest_platform(
    release: GitHubRelease,
    platform: Platform,
    manifest: &ReleaseManifest,
) -> Result<PlatformRelease, String> {
    let manifest_platform = manifest.platform(platform)?;
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == manifest_platform.archive)
        .ok_or_else(|| {
            format!(
                "Manifest {} references archive {}, but no matching GitHub asset was found",
                manifest.version, manifest_platform.archive
            )
        })?;

    Ok(PlatformRelease {
        version: manifest.version.clone(),
        name: release
            .name
            .unwrap_or_else(|| "Unnamed release".to_string()),
        html_url: release.html_url,
        metadata_source: ReleaseMetadataSource::Manifest,
        launch_options: manifest.launch_options.clone(),
        asset: ReleaseAsset {
            platform_id: platform.id().to_string(),
            name: manifest_platform.archive.clone(),
            download_url: asset.browser_download_url.clone(),
            size: manifest_platform.size,
            digest: Some(format!(
                "sha256:{}",
                normalize_sha256(&manifest_platform.sha256)
            )),
        },
    })
}

fn select_platform_asset_fallback(
    release: GitHubRelease,
    platform: Platform,
) -> Result<PlatformRelease, String> {
    let expected_asset_name = platform.release_asset_name(&release.tag_name);
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == expected_asset_name)
        .ok_or_else(|| {
            format!(
                "Release {} does not contain expected asset: {expected_asset_name}",
                release.tag_name
            )
        })?;

    Ok(PlatformRelease {
        version: release.tag_name,
        name: release
            .name
            .unwrap_or_else(|| "Unnamed release".to_string()),
        html_url: release.html_url,
        metadata_source: ReleaseMetadataSource::GitHubAssetFallback,
        launch_options: None,
        asset: ReleaseAsset {
            platform_id: platform.id().to_string(),
            name: asset.name,
            download_url: asset.browser_download_url,
            size: asset.size,
            digest: asset.digest,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_asset_matching_platform_naming_rule() {
        let release = GitHubRelease {
            tag_name: "V4".to_string(),
            name: Some("Dungeon Rampage Haxe V4".to_string()),
            html_url: "https://example.test/release".to_string(),
            assets: vec![
                GitHubAsset {
                    name: "Dungeon.Rampage.Haxe.V4.Windows.zip".to_string(),
                    browser_download_url: "https://example.test/windows.zip".to_string(),
                    size: 10,
                    digest: Some("sha256:windows".to_string()),
                },
                GitHubAsset {
                    name: "Dungeon.Rampage.Haxe.V4.Linux.tar.gz".to_string(),
                    browser_download_url: "https://example.test/linux.tar.gz".to_string(),
                    size: 20,
                    digest: Some("sha256:linux".to_string()),
                },
            ],
        };

        let selected = select_platform_release(release, Platform::LinuxX64, None).unwrap();

        assert_eq!(selected.version, "V4");
        assert_eq!(
            selected.metadata_source,
            ReleaseMetadataSource::GitHubAssetFallback
        );
        assert_eq!(selected.asset.name, "Dungeon.Rampage.Haxe.V4.Linux.tar.gz");
        assert_eq!(selected.asset.size, 20);
        assert_eq!(selected.asset.digest.as_deref(), Some("sha256:linux"));
        assert!(selected.launch_options.is_none());
    }

    #[test]
    fn reports_missing_platform_asset() {
        let release = GitHubRelease {
            tag_name: "V4".to_string(),
            name: None,
            html_url: "https://example.test/release".to_string(),
            assets: Vec::new(),
        };

        let error = select_platform_release(release, Platform::WindowsX64, None).unwrap_err();

        assert!(error.contains("Dungeon.Rampage.Haxe.V4.Windows.zip"));
    }

    #[test]
    fn selects_platform_from_manifest_when_available() {
        let release = GitHubRelease {
            tag_name: "V4".to_string(),
            name: Some("Dungeon Rampage Haxe V4".to_string()),
            html_url: "https://example.test/release".to_string(),
            assets: vec![GitHubAsset {
                name: "custom-linux.tar.gz".to_string(),
                browser_download_url: "https://example.test/custom-linux.tar.gz".to_string(),
                size: 999,
                digest: Some("sha256:ignored".to_string()),
            }],
        };
        let manifest = ReleaseManifest::parse(
            r#"{
                "version": "V4",
                "platforms": {
                    "linux-x64": {
                        "archive": "custom-linux.tar.gz",
                        "sha256": "manifest_hash",
                        "size": 123
                    }
                },
                "launch_options": {
                    "game_arguments": [
                        {
                            "name": "want-zoom",
                            "flag": "--want-zoom",
                            "default": false,
                            "recommended": true
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let selected =
            select_platform_release(release, Platform::LinuxX64, Some(&manifest)).unwrap();

        assert_eq!(selected.version, "V4");
        assert_eq!(selected.metadata_source, ReleaseMetadataSource::Manifest);
        assert_eq!(selected.asset.name, "custom-linux.tar.gz");
        assert_eq!(
            selected.asset.download_url,
            "https://example.test/custom-linux.tar.gz"
        );
        assert_eq!(selected.asset.size, 123);
        assert_eq!(
            selected.asset.digest.as_deref(),
            Some("sha256:manifest_hash")
        );
        assert_eq!(
            selected.launch_options.unwrap().game_arguments[0].recommended,
            Some(true)
        );
    }
}
