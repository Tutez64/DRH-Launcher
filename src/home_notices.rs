use std::sync::Mutex;

use crate::config::LauncherConfig;
use crate::steam_status::{self, SteamStatus};
use crate::{AppWindow, diagnostics, home_view, log_for_config};

static SELECTED_KIND: Mutex<Option<HomeNoticeKind>> = Mutex::new(None);
static LAST_LOGGED_KEYS: Mutex<String> = Mutex::new(String::new());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HomeNoticeKind {
    SteamMissing,
    SteamClosed,
    OfficialUpdate,
    Ownership,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HomeNotice {
    pub(crate) kind: HomeNoticeKind,
    pub(crate) text: String,
    pub(crate) detail: String,
    pub(crate) dismissible: bool,
}

impl HomeNoticeKind {
    fn log_key(self) -> &'static str {
        match self {
            Self::SteamMissing => "steam-missing",
            Self::SteamClosed => "steam-closed",
            Self::OfficialUpdate => "official-update",
            Self::Ownership => "ownership",
        }
    }
}

pub(crate) fn collect_home_notices(config: &LauncherConfig) -> Vec<HomeNotice> {
    notices_from(
        steam_status::cached_status(),
        &home_view::official_update_warning(config),
        config.hide_official_ownership_notice,
    )
}

pub(crate) fn apply_home_notices_view(ui: &AppWindow, config: &LauncherConfig) {
    let notices = collect_home_notices(config);
    let (index, selected) = select_visible_notice(&notices, *lock_selected_kind());
    *lock_selected_kind() = selected;

    match notices.get(index) {
        Some(notice) => {
            ui.set_home_notice_text(notice.text.clone().into());
            ui.set_home_notice_detail(notice.detail.clone().into());
            ui.set_home_notice_count(i32::try_from(notices.len()).unwrap_or(0));
            ui.set_home_notice_index(i32::try_from(index).unwrap_or(0));
            ui.set_home_notice_dismissible(notice.dismissible);
        }
        None => {
            ui.set_home_notice_text(String::new().into());
            ui.set_home_notice_detail(String::new().into());
            ui.set_home_notice_count(0);
            ui.set_home_notice_index(0);
            ui.set_home_notice_dismissible(false);
        }
    }

    log_notice_changes(config, &notices);
}

pub(crate) fn select_home_notice(index: i32, config: &LauncherConfig, ui: &AppWindow) {
    let notices = collect_home_notices(config);
    if let Some(notice) = usize::try_from(index)
        .ok()
        .and_then(|index| notices.get(index))
    {
        *lock_selected_kind() = Some(notice.kind);
    }
    apply_home_notices_view(ui, config);
}

pub(crate) fn dismiss_current_home_notice(config: &mut LauncherConfig, ui: &AppWindow) {
    if *lock_selected_kind() == Some(HomeNoticeKind::Ownership) {
        config.hide_official_ownership_notice = true;
        if let Err(error) = config.save() {
            log_for_config(
                config,
                diagnostics::LogLevel::Error,
                &format!("Could not hide the official Dungeon Rampage notice: {error}"),
            );
        } else {
            log_for_config(
                config,
                diagnostics::LogLevel::Info,
                "The official Dungeon Rampage notice is now hidden.",
            );
        }
    }
    apply_home_notices_view(ui, config);
}

fn notices_from(
    steam: SteamStatus,
    official_update: &str,
    ownership_dismissed: bool,
) -> Vec<HomeNotice> {
    let mut notices = Vec::new();

    if !steam.steam_installed {
        notices.push(steam_missing_notice());
    } else if !steam.steam_running {
        notices.push(steam_closed_notice());
    }

    if !official_update.is_empty() {
        notices.push(official_update_notice(official_update));
    }

    if steam.steam_installed && !steam.official_game_installed && !ownership_dismissed {
        notices.push(ownership_notice());
    }

    notices
}

fn steam_missing_notice() -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::SteamMissing,
        text: "Steam is missing. Install it so DRH can connect to the official servers.".to_string(),
        detail: "DRH connects to the official Dungeon Rampage servers via Steam.\nIt is NOT a crack. Install Steam and make sure you own Dungeon Rampage.".to_string(),
        dismissible: false,
    }
}

fn steam_closed_notice() -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::SteamClosed,
        text: "Steam is not running. Launch it so DRH can connect to the official servers.".to_string(),
        detail: "DRH connects to the official Dungeon Rampage servers via Steam.\nIt is NOT a crack. Leave Steam running and logged in to play.".to_string(),
        dismissible: false,
    }
}

fn official_update_notice(detail: &str) -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::OfficialUpdate,
        text: "The official Dungeon Rampage was updated. This DRH version may no longer work."
            .to_string(),
        detail: detail.to_string(),
        dismissible: false,
    }
}

fn ownership_notice() -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::Ownership,
        text: "You need to own the official Dungeon Rampage for DRH to connect.".to_string(),
        detail: "If you don't own Dungeon Rampage, DRH won't be able to connect to the official servers. We couldn't find it installed, but that doesn't mean you don't own it, and you do not need to install it.\nTo dismiss this message, click on \"Hide\" or disable it in the Settings.".to_string(),
        dismissible: true,
    }
}

fn select_visible_notice(
    notices: &[HomeNotice],
    selected: Option<HomeNoticeKind>,
) -> (usize, Option<HomeNoticeKind>) {
    if notices.is_empty() {
        return (0, None);
    }
    let index = notices
        .iter()
        .position(|notice| Some(notice.kind) == selected)
        .unwrap_or(0);
    (index, Some(notices[index].kind))
}

fn log_notice_changes(config: &LauncherConfig, notices: &[HomeNotice]) {
    let keys = notices
        .iter()
        .map(|notice| format!("{}:{}", notice.kind.log_key(), notice.detail))
        .collect::<Vec<_>>()
        .join("|");
    let mut last = lock_last_logged_keys();
    if keys == *last {
        return;
    }
    if notices.is_empty() {
        log_for_config(
            config,
            diagnostics::LogLevel::Info,
            "Home Steam notices cleared.",
        );
    } else {
        for notice in notices {
            log_for_config(config, diagnostics::LogLevel::Warn, &notice.detail);
        }
    }
    *last = keys;
}

fn lock_selected_kind() -> std::sync::MutexGuard<'static, Option<HomeNoticeKind>> {
    SELECTED_KIND
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_last_logged_keys() -> std::sync::MutexGuard<'static, String> {
    LAST_LOGGED_KEYS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steam(installed: bool, running: bool, official_installed: bool) -> SteamStatus {
        SteamStatus {
            steam_installed: installed,
            steam_running: running,
            official_game_installed: official_installed,
        }
    }

    #[test]
    fn steam_missing_does_not_also_warn_about_ownership() {
        let notices = notices_from(steam(false, false, false), "", false);
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, HomeNoticeKind::SteamMissing);
        assert!(notices[0].text.contains("Steam is missing"));
        assert!(notices[0].detail.contains("NOT a crack"));
        assert!(notices[0].detail.contains("own Dungeon Rampage"));
    }

    #[test]
    fn steam_closed_outranks_ownership_and_official_update() {
        let notices = notices_from(
            steam(true, false, false),
            "The official Dungeon Rampage was updated. Wait for the next DRH release.",
            false,
        );
        assert_eq!(
            notices.iter().map(|notice| notice.kind).collect::<Vec<_>>(),
            vec![
                HomeNoticeKind::SteamClosed,
                HomeNoticeKind::OfficialUpdate,
                HomeNoticeKind::Ownership,
            ]
        );
        assert!(!notices[0].dismissible);
        assert!(notices[2].dismissible);
    }

    #[test]
    fn ownership_notice_is_skipped_when_official_client_is_installed() {
        let notices = notices_from(steam(true, true, true), "", false);
        assert!(notices.is_empty());
    }

    #[test]
    fn ownership_notice_can_be_hidden() {
        let visible = notices_from(steam(true, true, false), "", false);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].kind, HomeNoticeKind::Ownership);
        assert!(visible[0].text.contains("own the official Dungeon Rampage"));
        assert!(visible[0].detail.contains("doesn't mean you don't own it"));
        assert!(visible[0].detail.contains("Settings → General"));

        let dismissed = notices_from(steam(true, true, false), "", true);
        assert!(dismissed.is_empty());
    }

    #[test]
    fn keeps_selected_kind_when_a_higher_priority_notice_disappears() {
        let notices = notices_from(steam(true, true, false), "", false);
        let (index, selected) = select_visible_notice(&notices, Some(HomeNoticeKind::Ownership));
        assert_eq!(index, 0);
        assert_eq!(selected, Some(HomeNoticeKind::Ownership));

        let remaining = notices_from(steam(true, true, false), "", false);
        let (index, selected) =
            select_visible_notice(&remaining, Some(HomeNoticeKind::SteamClosed));
        assert_eq!(index, 0);
        assert_eq!(selected, Some(HomeNoticeKind::Ownership));
    }
}
