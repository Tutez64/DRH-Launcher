#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallState {
    NotInstalled,
    Installed,
    UpdateAvailable,
    Updating,
    Playing,
    BrokenInstall,
    LaunchableButMaybeOutdated,
}

impl InstallState {
    pub fn primary_action(&self) -> &'static str {
        match self {
            Self::NotInstalled => "Install DRH",
            Self::Installed | Self::LaunchableButMaybeOutdated => "Play",
            Self::UpdateAvailable => "Update",
            Self::Updating => "Updating...",
            Self::Playing => "Stop",
            Self::BrokenInstall => "Repair",
        }
    }

    pub fn status_text(&self) -> &'static str {
        match self {
            Self::NotInstalled => "Not installed",
            Self::Installed => "Ready",
            Self::UpdateAvailable => "Update available",
            Self::Updating => "Updating...",
            Self::Playing => "Running",
            Self::BrokenInstall => "Install incomplete",
            Self::LaunchableButMaybeOutdated => "Update status unknown",
        }
    }
}
