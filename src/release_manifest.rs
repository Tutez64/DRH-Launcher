use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::platform::Platform;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReleaseManifest {
    pub version: String,
    pub platforms: HashMap<String, ManifestPlatform>,
    #[serde(default)]
    pub launch_options: Option<ManifestLaunchOptions>,
}

impl ReleaseManifest {
    pub fn parse(contents: &str) -> Result<Self, String> {
        serde_json::from_str(contents)
            .map_err(|error| format!("Could not parse release manifest: {error}"))
    }

    pub fn platform(&self, platform: Platform) -> Result<&ManifestPlatform, String> {
        self.platforms.get(platform.id()).ok_or_else(|| {
            format!(
                "Manifest {} does not define platform {}",
                self.version,
                platform.id()
            )
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestPlatform {
    pub archive: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestLaunchOptions {
    #[serde(default)]
    pub game_arguments: Vec<ManifestGameArgument>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestGameArgument {
    pub name: String,
    pub flag: String,
    pub default: bool,
    #[serde(default)]
    pub recommended: Option<bool>,
    #[serde(default)]
    pub config_key: Option<String>,
}

pub fn is_manifest_asset_name(name: &str, version: &str) -> bool {
    name == "manifest.json" || name == format!("Dungeon.Rampage.Haxe.{version}.manifest.json")
}

pub fn normalize_sha256(value: &str) -> String {
    value
        .strip_prefix("sha256:")
        .unwrap_or(value)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_and_selects_platform() {
        let manifest = ReleaseManifest::parse(
            r#"{
                "version": "V3",
                "platforms": {
                    "linux-x64": {
                        "archive": "Dungeon.Rampage.Haxe.V3.Linux.tar.gz",
                        "sha256": "abc123",
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
                        },
                        {
                            "name": "quality-control-button",
                            "flag": "--quality-control-button",
                            "default": true,
                            "config_key": "quality-control-button"
                        }
                    ]
                }
            }"#,
        )
        .unwrap();

        let platform = manifest.platform(Platform::LinuxX64).unwrap();

        assert_eq!(manifest.version, "V3");
        assert_eq!(platform.archive, "Dungeon.Rampage.Haxe.V3.Linux.tar.gz");
        assert_eq!(platform.sha256, "abc123");
        assert_eq!(platform.size, 123);
        let launch_options = manifest.launch_options.unwrap();
        assert_eq!(launch_options.game_arguments.len(), 2);
        assert_eq!(launch_options.game_arguments[0].flag, "--want-zoom");
        assert_eq!(launch_options.game_arguments[0].recommended, Some(true));
        assert_eq!(
            launch_options.game_arguments[1].config_key.as_deref(),
            Some("quality-control-button")
        );
    }

    #[test]
    fn recognizes_manifest_asset_names() {
        assert!(is_manifest_asset_name("manifest.json", "V4"));
        assert!(is_manifest_asset_name(
            "Dungeon.Rampage.Haxe.V4.manifest.json",
            "V4"
        ));
        assert!(!is_manifest_asset_name(
            "Dungeon.Rampage.Haxe.V4.Linux.tar.gz",
            "V4"
        ));
    }

    #[test]
    fn normalizes_sha256_values() {
        assert_eq!(normalize_sha256("sha256:abc123"), "abc123");
        assert_eq!(normalize_sha256("abc123"), "abc123");
    }
}
