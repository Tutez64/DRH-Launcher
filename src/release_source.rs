use std::env;

#[derive(Clone, Debug)]
pub struct ReleaseSource {
    pub owner: &'static str,
    pub repo: &'static str,
}

impl ReleaseSource {
    pub fn drh() -> Self {
        Self {
            owner: "Tutez64",
            repo: "Dungeon-Rampage-Haxe",
        }
    }

    pub fn fixtures() -> Self {
        Self {
            owner: "Tutez64",
            repo: "DRHL-Release-Fixtures",
        }
    }

    pub fn launcher() -> Self {
        Self {
            owner: "Tutez64",
            repo: "DRH-Launcher",
        }
    }

    pub fn from_environment() -> Self {
        match env::var("DRHL_RELEASE_SOURCE").as_deref() {
            Ok("fixtures") => Self::fixtures(),
            Ok("drh") => Self::drh(),
            _ => Self::default_for_build(),
        }
    }

    fn default_for_build() -> Self {
        if cfg!(debug_assertions) {
            Self::fixtures()
        } else {
            Self::drh()
        }
    }

    pub fn label(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    pub fn api_latest_release_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.owner, self.repo
        )
    }

    pub fn api_releases_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases",
            self.owner, self.repo
        )
    }

    pub fn api_release_by_tag_url(&self, tag: &str) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/tags/{}",
            self.owner, self.repo, tag
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_latest_release_api_url() {
        let source = ReleaseSource::fixtures();

        assert_eq!(
            source.api_latest_release_url(),
            "https://api.github.com/repos/Tutez64/DRHL-Release-Fixtures/releases/latest"
        );
    }

    #[test]
    fn builds_release_history_api_urls() {
        let source = ReleaseSource::launcher();

        assert_eq!(
            source.api_releases_url(),
            "https://api.github.com/repos/Tutez64/DRH-Launcher/releases"
        );
        assert_eq!(
            source.api_release_by_tag_url("v1.2.3"),
            "https://api.github.com/repos/Tutez64/DRH-Launcher/releases/tags/v1.2.3"
        );
    }

    #[test]
    fn debug_build_defaults_to_fixtures() {
        let source = ReleaseSource::default_for_build();

        if cfg!(debug_assertions) {
            assert_eq!(source.repo, "DRHL-Release-Fixtures");
        } else {
            assert_eq!(source.repo, "Dungeon-Rampage-Haxe");
        }
    }
}
