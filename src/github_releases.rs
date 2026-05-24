use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, USER_AGENT};
use serde::Deserialize;
use std::time::Duration;

use crate::platform::Platform;
use crate::release_source::ReleaseSource;

#[derive(Clone, Debug)]
pub struct PlatformRelease {
    pub version: String,
    pub name: String,
    pub html_url: String,
    pub asset: ReleaseAsset,
}

impl PlatformRelease {
    pub fn summary(&self) -> String {
        let digest = self
            .asset
            .digest
            .as_deref()
            .unwrap_or("no GitHub digest available");
        format!(
            "Latest {} release: {} / {} ({}, {} bytes, {digest}). Release: {}. Download: {}",
            self.asset.platform_id,
            self.version,
            self.name,
            self.asset.name,
            self.asset.size,
            self.html_url,
            self.asset.download_url
        )
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

    select_platform_asset(release, platform)
}

fn select_platform_asset(
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

        let selected = select_platform_asset(release, Platform::LinuxX64).unwrap();

        assert_eq!(selected.version, "V4");
        assert_eq!(selected.asset.name, "Dungeon.Rampage.Haxe.V4.Linux.tar.gz");
        assert_eq!(selected.asset.size, 20);
        assert_eq!(selected.asset.digest.as_deref(), Some("sha256:linux"));
    }

    #[test]
    fn reports_missing_platform_asset() {
        let release = GitHubRelease {
            tag_name: "V4".to_string(),
            name: None,
            html_url: "https://example.test/release".to_string(),
            assets: Vec::new(),
        };

        let error = select_platform_asset(release, Platform::WindowsX64).unwrap_err();

        assert!(error.contains("Dungeon.Rampage.Haxe.V4.Windows.zip"));
    }
}
