use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::LauncherConfig;
use crate::steam_buildid::STEAM_STORE_CLIENT_URL;
use crate::steam_status::{self, SteamStatus};
use crate::{AppWindow, diagnostics, home_view, log_for_config, steam_start};

static SELECTED_KIND: Mutex<Option<HomeNoticeKind>> = Mutex::new(None);
static LAST_LOGGED_KEYS: Mutex<String> = Mutex::new(String::new());
/// Home is asking whether Steam should start with the launcher, until the
/// player answers or the launcher closes.
static STEAM_AUTO_START_OFFERED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HomeNoticeKind {
    SteamMissing,
    SteamClosed,
    SteamAutoStartOffer,
    OfficialUpdate,
    Ownership,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HomeNotice {
    pub(crate) kind: HomeNoticeKind,
    pub(crate) text: String,
    pub(crate) detail: String,
    pub(crate) dismissible: bool,
    pub(crate) link: Option<HomeNoticeLink>,
    pub(crate) action: Option<HomeNoticeAction>,
}

/// What the button of a notice does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HomeNoticeAction {
    StartSteam,
    EnableSteamAutoStart,
}

impl HomeNoticeAction {
    fn label(self) -> &'static str {
        match self {
            Self::StartSteam => "Start Steam",
            Self::EnableSteamAutoStart => "Yes",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HomeNoticeLink {
    pub(crate) prefix: String,
    pub(crate) label: String,
    pub(crate) suffix: String,
    pub(crate) url: String,
}

impl HomeNoticeKind {
    fn log_key(self) -> &'static str {
        match self {
            Self::SteamMissing => "steam-missing",
            Self::SteamClosed => "steam-closed",
            Self::SteamAutoStartOffer => "steam-auto-start-offer",
            Self::OfficialUpdate => "official-update",
            Self::Ownership => "ownership",
        }
    }

    fn dismiss_label(self) -> &'static str {
        match self {
            Self::SteamAutoStartOffer => "No",
            _ => "Hide",
        }
    }
}

pub(crate) fn collect_home_notices(config: &LauncherConfig) -> Vec<HomeNotice> {
    collect_notices(config, STEAM_AUTO_START_OFFERED.load(Ordering::Relaxed))
}

fn collect_notices(config: &LauncherConfig, steam_auto_start_offered: bool) -> Vec<HomeNotice> {
    notices_from(
        steam_status::cached_status(),
        steam_start::is_starting(),
        steam_auto_start_offered,
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
            ui.set_home_notice_dismiss_label(notice.kind.dismiss_label().into());
            ui.set_home_notice_action_label(
                notice
                    .action
                    .map(HomeNoticeAction::label)
                    .unwrap_or_default()
                    .into(),
            );
            apply_notice_link(ui, notice.link.as_ref());
        }
        None => {
            ui.set_home_notice_text(String::new().into());
            ui.set_home_notice_detail(String::new().into());
            ui.set_home_notice_count(0);
            ui.set_home_notice_index(0);
            ui.set_home_notice_dismissible(false);
            ui.set_home_notice_action_label(String::new().into());
            apply_notice_link(ui, None);
        }
    }

    // The log follows the problems, not the question asked about Steam,
    // which is none and hides the Steam notice while Steam starts.
    if notices
        .iter()
        .any(|notice| notice.kind == HomeNoticeKind::SteamAutoStartOffer)
    {
        log_notice_changes(config, &collect_notices(config, false));
    } else {
        log_notice_changes(config, &notices);
    }
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

/// The action of the notice Home shows, when it has one.
pub(crate) fn current_home_notice_action(config: &LauncherConfig) -> Option<HomeNoticeAction> {
    let notices = collect_home_notices(config);
    let (index, _) = select_visible_notice(&notices, *lock_selected_kind());
    notices.get(index).and_then(|notice| notice.action)
}

/// Has Home ask, with the next notices, whether Steam should start with the
/// launcher. Asked once: not to a player who answered, or who has it on.
pub(crate) fn offer_steam_auto_start(config: &LauncherConfig) {
    if !config.start_steam_with_launcher && !config.start_steam_offer_answered {
        STEAM_AUTO_START_OFFERED.store(true, Ordering::Relaxed);
    }
}

/// Takes the question back without an answer, to ask it another time.
pub(crate) fn withdraw_steam_auto_start_offer() {
    STEAM_AUTO_START_OFFERED.store(false, Ordering::Relaxed);
}

pub(crate) fn answer_steam_auto_start_offer(
    config: &mut LauncherConfig,
    enable: bool,
    ui: &AppWindow,
) {
    withdraw_steam_auto_start_offer();
    config.start_steam_offer_answered = true;
    if enable {
        config.start_steam_with_launcher = true;
    }
    match config.save() {
        Ok(()) => log_for_config(
            config,
            diagnostics::LogLevel::Info,
            if enable {
                "Steam will be started each time the launcher opens."
            } else {
                "Starting Steam with the launcher was declined."
            },
        ),
        Err(error) => log_for_config(
            config,
            diagnostics::LogLevel::Error,
            &format!("Could not save the answer about starting Steam with the launcher: {error}"),
        ),
    }
    apply_home_notices_view(ui, config);
}

pub(crate) fn dismiss_current_home_notice(config: &mut LauncherConfig, ui: &AppWindow) {
    let selected = *lock_selected_kind();
    if selected == Some(HomeNoticeKind::SteamAutoStartOffer) {
        answer_steam_auto_start_offer(config, false, ui);
        return;
    }
    if selected == Some(HomeNoticeKind::Ownership) {
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
    steam_starting: bool,
    steam_auto_start_offered: bool,
    official_update: &str,
    ownership_dismissed: bool,
) -> Vec<HomeNotice> {
    let mut notices = Vec::new();

    if steam.probed {
        if !steam.steam_installed {
            notices.push(steam_missing_notice());
        } else if !steam.steam_running {
            // While Steam starts, the question says so itself, in place of
            // the notice it was asked from.
            let said_by_offer = steam_starting && steam_auto_start_offered;
            if !said_by_offer {
                notices.push(steam_closed_notice(steam_starting));
            }
        }
    }

    if steam_auto_start_offered {
        notices.push(steam_auto_start_offer_notice(steam_starting));
    }

    if !official_update.is_empty() {
        notices.push(official_update_notice(official_update));
    }

    if steam.probed
        && steam.steam_installed
        && !steam.official_game_installed
        && !ownership_dismissed
    {
        notices.push(ownership_notice());
    }

    notices
}

fn apply_notice_link(ui: &AppWindow, link: Option<&HomeNoticeLink>) {
    match link {
        Some(link) => {
            ui.set_home_notice_has_link(true);
            ui.set_home_notice_link_prefix(link.prefix.clone().into());
            ui.set_home_notice_link_label(link.label.clone().into());
            ui.set_home_notice_link_suffix(link.suffix.clone().into());
            ui.set_home_notice_link_url(link.url.clone().into());
        }
        None => {
            ui.set_home_notice_has_link(false);
            ui.set_home_notice_link_prefix(String::new().into());
            ui.set_home_notice_link_label(String::new().into());
            ui.set_home_notice_link_suffix(String::new().into());
            ui.set_home_notice_link_url(String::new().into());
        }
    }
}

fn steam_missing_notice() -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::SteamMissing,
        text: "Steam is missing. Install it so DRH can connect to the official servers.".to_string(),
        detail: "DRH connects to the official Dungeon Rampage servers via Steam.\nIt is NOT a crack. Install Steam and make sure you own Dungeon Rampage.".to_string(),
        dismissible: false,
        link: None,
        action: None,
    }
}

/// The detail stays the same while Steam starts, so that the log, which
/// follows the details, does not warn a second time.
fn steam_closed_notice(starting: bool) -> HomeNotice {
    let (text, action) = if starting {
        ("Starting Steam…", None)
    } else {
        (
            "Steam is not running. DRH needs it to connect to the official servers.",
            Some(HomeNoticeAction::StartSteam),
        )
    };
    HomeNotice {
        kind: HomeNoticeKind::SteamClosed,
        text: text.to_string(),
        detail: "DRH connects to the official Dungeon Rampage servers via Steam.\nIt is NOT a crack. Leave Steam running and logged in to play.\nThe launcher can start Steam each time it opens: see Settings.".to_string(),
        dismissible: false,
        link: None,
        action,
    }
}

fn steam_auto_start_offer_notice(starting: bool) -> HomeNotice {
    let text = if starting {
        "Starting Steam… Do it each time the launcher opens?"
    } else {
        "Start Steam each time the launcher opens?"
    };
    HomeNotice {
        kind: HomeNoticeKind::SteamAutoStartOffer,
        text: text.to_string(),
        detail: "The launcher can start Steam in the background each time it opens, when it is not running.\nThis can be changed in Settings.".to_string(),
        dismissible: true,
        link: None,
        action: Some(HomeNoticeAction::EnableSteamAutoStart),
    }
}

fn official_update_notice(detail: &str) -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::OfficialUpdate,
        text: "The official Dungeon Rampage was updated. This DRH version may no longer work."
            .to_string(),
        detail: detail.to_string(),
        dismissible: false,
        link: None,
        action: None,
    }
}

fn ownership_notice() -> HomeNotice {
    HomeNotice {
        kind: HomeNoticeKind::Ownership,
        text: "You need to own the official Dungeon Rampage for DRH to connect.".to_string(),
        detail: "If you don't own Dungeon Rampage, DRH won't be able to connect to the official servers. We couldn't find it installed, but that doesn't mean you don't own it, and you do not need to install it.\nTo dismiss this message, click on \"Hide\" or disable it in the Settings.".to_string(),
        dismissible: true,
        link: Some(HomeNoticeLink {
            prefix: "You need to own ".to_string(),
            label: "the official Dungeon Rampage".to_string(),
            suffix: " for DRH to connect.".to_string(),
            url: STEAM_STORE_CLIENT_URL.to_string(),
        }),
        action: None,
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
            probed: true,
            steam_installed: installed,
            steam_running: running,
            official_game_installed: official_installed,
        }
    }

    fn unprobed() -> SteamStatus {
        SteamStatus {
            probed: false,
            steam_installed: false,
            steam_running: false,
            official_game_installed: false,
        }
    }

    #[test]
    fn unprobed_status_does_not_show_steam_notices() {
        let notices = notices_from(
            unprobed(),
            false,
            false,
            "The official Dungeon Rampage was updated. Wait for the next DRH release.",
            false,
        );
        assert_eq!(
            notices.iter().map(|notice| notice.kind).collect::<Vec<_>>(),
            vec![HomeNoticeKind::OfficialUpdate]
        );
    }

    #[test]
    fn steam_missing_does_not_also_warn_about_ownership() {
        let notices = notices_from(steam(false, false, false), false, false, "", false);
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
            false,
            false,
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
    fn steam_closed_offers_to_start_steam_until_it_is_starting() {
        let closed = notices_from(steam(true, false, true), false, false, "", false);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].action, Some(HomeNoticeAction::StartSteam));
        assert!(closed[0].text.contains("Steam is not running"));

        let starting = notices_from(steam(true, false, true), true, false, "", false);
        assert_eq!(starting.len(), 1);
        assert_eq!(starting[0].kind, HomeNoticeKind::SteamClosed);
        assert_eq!(starting[0].action, None);
        assert_eq!(starting[0].text, "Starting Steam…");
        assert_eq!(starting[0].detail, closed[0].detail);

        let missing = notices_from(steam(false, false, false), true, false, "", false);
        assert_eq!(missing[0].action, None);
    }

    #[test]
    fn auto_start_offer_stands_for_the_steam_notice_while_steam_starts() {
        let kinds =
            |notices: &[HomeNotice]| notices.iter().map(|notice| notice.kind).collect::<Vec<_>>();

        let starting = notices_from(steam(true, false, true), true, true, "", false);
        assert_eq!(kinds(&starting), vec![HomeNoticeKind::SteamAutoStartOffer]);
        assert!(starting[0].text.starts_with("Starting Steam…"));
        assert_eq!(
            starting[0].action,
            Some(HomeNoticeAction::EnableSteamAutoStart)
        );
        assert!(starting[0].dismissible);
        assert_eq!(starting[0].kind.dismiss_label(), "No");

        let running = notices_from(steam(true, true, true), false, true, "", false);
        assert_eq!(kinds(&running), vec![HomeNoticeKind::SteamAutoStartOffer]);
        assert_eq!(running[0].text, "Start Steam each time the launcher opens?");

        let not_started = notices_from(steam(true, false, true), false, true, "", false);
        assert_eq!(
            kinds(&not_started),
            vec![
                HomeNoticeKind::SteamClosed,
                HomeNoticeKind::SteamAutoStartOffer
            ]
        );
        let (index, selected) =
            select_visible_notice(&not_started, Some(HomeNoticeKind::SteamAutoStartOffer));
        assert_eq!(index, 1);
        assert_eq!(selected, Some(HomeNoticeKind::SteamAutoStartOffer));
    }

    #[test]
    fn auto_start_is_offered_once_and_not_when_already_on() {
        let mut config = LauncherConfig::default();
        withdraw_steam_auto_start_offer();

        config.start_steam_with_launcher = true;
        offer_steam_auto_start(&config);
        assert!(!STEAM_AUTO_START_OFFERED.load(Ordering::Relaxed));

        config.start_steam_with_launcher = false;
        config.start_steam_offer_answered = true;
        offer_steam_auto_start(&config);
        assert!(!STEAM_AUTO_START_OFFERED.load(Ordering::Relaxed));

        config.start_steam_offer_answered = false;
        offer_steam_auto_start(&config);
        assert!(STEAM_AUTO_START_OFFERED.load(Ordering::Relaxed));
        withdraw_steam_auto_start_offer();
    }

    #[test]
    fn ownership_notice_is_skipped_when_official_client_is_installed() {
        let notices = notices_from(steam(true, true, true), false, false, "", false);
        assert!(notices.is_empty());
    }

    #[test]
    fn ownership_notice_can_be_hidden() {
        let visible = notices_from(steam(true, true, false), false, false, "", false);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].kind, HomeNoticeKind::Ownership);
        assert_eq!(
            visible[0].text,
            "You need to own the official Dungeon Rampage for DRH to connect."
        );
        let link = visible[0]
            .link
            .as_ref()
            .expect("ownership notice has a store link");
        assert_eq!(link.label, "the official Dungeon Rampage");
        assert_eq!(link.url, STEAM_STORE_CLIENT_URL);
        assert!(visible[0].detail.contains("doesn't mean you don't own it"));

        let dismissed = notices_from(steam(true, true, false), false, false, "", true);
        assert!(dismissed.is_empty());
    }

    #[test]
    fn keeps_selected_kind_when_a_higher_priority_notice_disappears() {
        let notices = notices_from(steam(true, true, false), false, false, "", false);
        let (index, selected) = select_visible_notice(&notices, Some(HomeNoticeKind::Ownership));
        assert_eq!(index, 0);
        assert_eq!(selected, Some(HomeNoticeKind::Ownership));

        let remaining = notices_from(steam(true, true, false), false, false, "", false);
        let (index, selected) =
            select_visible_notice(&remaining, Some(HomeNoticeKind::SteamClosed));
        assert_eq!(index, 0);
        assert_eq!(selected, Some(HomeNoticeKind::Ownership));
    }
}
