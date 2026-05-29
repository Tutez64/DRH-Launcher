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
            Self::NotInstalled => "DRH is not installed",
            Self::Installed => "DRH is installed",
            Self::UpdateAvailable => "A DRH update is available",
            Self::Updating => "DRH is updating",
            Self::Playing => "DRH is running",
            Self::BrokenInstall => "DRH installation is incomplete",
            Self::LaunchableButMaybeOutdated => "DRH is launchable, but update status is unknown",
        }
    }
}
