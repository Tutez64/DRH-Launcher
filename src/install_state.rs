#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
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
}
