#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    LinuxX64,
    WindowsX64,
    MacosUniversal,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::WindowsX64
        } else if cfg!(target_os = "macos") {
            Self::MacosUniversal
        } else {
            Self::LinuxX64
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::LinuxX64 => "linux-x64",
            Self::WindowsX64 => "windows-x64",
            Self::MacosUniversal => "macos-universal",
        }
    }

    pub fn release_asset_name(&self, version: &str) -> String {
        match self {
            Self::LinuxX64 => format!("Dungeon.Rampage.Haxe.{version}.Linux.tar.gz"),
            Self::WindowsX64 => format!("Dungeon.Rampage.Haxe.{version}.Windows.zip"),
            Self::MacosUniversal => format!("Dungeon.Rampage.Haxe.{version}.macOS.zip"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_expected_release_asset_names() {
        assert_eq!(
            Platform::LinuxX64.release_asset_name("V3"),
            "Dungeon.Rampage.Haxe.V3.Linux.tar.gz"
        );
        assert_eq!(
            Platform::WindowsX64.release_asset_name("V3"),
            "Dungeon.Rampage.Haxe.V3.Windows.zip"
        );
        assert_eq!(
            Platform::MacosUniversal.release_asset_name("V3"),
            "Dungeon.Rampage.Haxe.V3.macOS.zip"
        );
    }
}
