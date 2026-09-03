use std::sync::Mutex;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::USER_AGENT;
use serde::Deserialize;

use crate::steam_buildid::STEAM_APP_ID;

const HTTP_USER_AGENT: &str = "DRH-Launcher";

static PLAYER_COUNT: Mutex<Option<u32>> = Mutex::new(None);

pub fn cached_player_count() -> Option<u32> {
    *lock_player_count()
}

pub fn cache_player_count(count: u32) {
    *lock_player_count() = Some(count);
}

pub fn players_text(count: u32) -> String {
    match count {
        1 => "1 player in Dungeon Rampage".to_string(),
        n => format!("{n} players in Dungeon Rampage"),
    }
}

pub fn players_detail_text(count: u32) -> String {
    let lead = match count {
        0 => "There are no players in Dungeon Rampage (official and DRH).".to_string(),
        1 => "There is 1 player in Dungeon Rampage (official and DRH).".to_string(),
        n => format!("There are {n} players in Dungeon Rampage (official and DRH)."),
    };
    format!("{lead}\nSteam refreshes this about every five minutes.")
}

pub fn fetch_player_count() -> Result<u32, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not create Steam player-count client: {error}"))?;
    let response = client
        .get(player_count_url())
        .header(USER_AGENT, HTTP_USER_AGENT)
        .send()
        .map_err(|error| format!("Could not check Steam player count: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Steam player count request failed: {error}"))?
        .json::<PlayerCountResponse>()
        .map_err(|error| format!("Could not parse Steam player count: {error}"))?;

    parse_player_count(&response)
        .ok_or_else(|| "Steam player count was missing from the response.".to_string())
}

fn player_count_url() -> String {
    format!(
        "https://api.steampowered.com/ISteamUserStats/GetNumberOfCurrentPlayers/v1/?appid={STEAM_APP_ID}"
    )
}

fn lock_player_count() -> std::sync::MutexGuard<'static, Option<u32>> {
    PLAYER_COUNT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn parse_player_count(response: &PlayerCountResponse) -> Option<u32> {
    if response.response.result.unwrap_or_default() != 1 {
        return None;
    }
    response.response.player_count
}

#[derive(Debug, Deserialize)]
struct PlayerCountResponse {
    response: PlayerCountInner,
}

#[derive(Debug, Deserialize)]
struct PlayerCountInner {
    #[serde(default)]
    player_count: Option<u32>,
    #[serde(default)]
    result: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_successful_player_count() {
        let players: PlayerCountResponse =
            serde_json::from_str(r#"{ "response": { "player_count": 49, "result": 1 } }"#).unwrap();
        assert_eq!(parse_player_count(&players), Some(49));

        let zero: PlayerCountResponse =
            serde_json::from_str(r#"{ "response": { "player_count": 0, "result": 1 } }"#).unwrap();
        assert_eq!(parse_player_count(&zero), Some(0));
    }

    #[test]
    fn rejects_unsuccessful_or_incomplete_payloads() {
        let failed: PlayerCountResponse =
            serde_json::from_str(r#"{ "response": { "player_count": 49, "result": 42 } }"#)
                .unwrap();
        assert!(parse_player_count(&failed).is_none());

        let missing_count: PlayerCountResponse =
            serde_json::from_str(r#"{ "response": { "result": 1 } }"#).unwrap();
        assert!(parse_player_count(&missing_count).is_none());

        let missing_result: PlayerCountResponse =
            serde_json::from_str(r#"{ "response": { "player_count": 49 } }"#).unwrap();
        assert!(parse_player_count(&missing_result).is_none());
    }

    #[test]
    fn player_count_url_targets_dungeon_rampage() {
        assert!(player_count_url().contains("GetNumberOfCurrentPlayers"));
        assert!(player_count_url().ends_with("appid=3053950"));
    }

    #[test]
    fn formats_player_count_copy() {
        assert_eq!(players_text(0), "0 players in Dungeon Rampage");
        assert_eq!(players_text(1), "1 player in Dungeon Rampage");
        assert_eq!(players_text(49), "49 players in Dungeon Rampage");

        assert_eq!(
            players_detail_text(0),
            "There are no players in Dungeon Rampage (official and DRH).\nSteam refreshes this about every five minutes."
        );
        assert_eq!(
            players_detail_text(1),
            "There is 1 player in Dungeon Rampage (official and DRH).\nSteam refreshes this about every five minutes."
        );
        assert_eq!(
            players_detail_text(141),
            "There are 141 players in Dungeon Rampage (official and DRH).\nSteam refreshes this about every five minutes."
        );
    }
}
