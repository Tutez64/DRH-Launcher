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

    pub fn api_latest_release_url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            self.owner, self.repo
        )
    }
}
