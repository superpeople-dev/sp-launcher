//! The Leaderboard page: the season's top 100 of each mode, Solo to Squad in
//! TPP and FPP, as the lobby and superpeople.dev/leaderboard show it. From the
//! game backend's public route (sp-backend GET /ds/api/listen/public/leaderboard:
//! rank, in-game name, ranked points, tier and country, nothing else), asked
//! here because the webview's CSP allows no outside hosts. Flags are the
//! website's images (superpeople.dev/flags/<cc>.svg): emoji flags don't show
//! on Windows.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::auth::{client, send, site_url, OOPS};
use crate::error::{LauncherError, Result};

/// The same backend as `auth::AUTH_BASE_URL`, its public part.
const URL: &str = "http://64.226.112.204:8080/ds/api/listen/public/leaderboard";

/// The lists, in the game's order.
pub const KEYS: [&str; 8] = ["solo_tpp", "solo_fpp", "duo_tpp", "duo_fpp", "trio_tpp", "trio_fpp", "squad_tpp", "squad_fpp"];

#[derive(Debug, Clone, Deserialize)]
struct RawRow {
    #[serde(default)]
    rank: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    rp: f64,
    #[serde(default)]
    tier: u64,
    #[serde(default)]
    country: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawBoard {
    #[serde(default)]
    updated: u64,
    #[serde(default)]
    modes: BTreeMap<String, Vec<RawRow>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Envelope {
    d: Option<RawBoard>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Row {
    pub rank: u32,
    pub name: String,
    pub rp: i64,
    /// The game's tier id (420100001 Super Soldier .. 420100034 Iron V).
    pub tier: u64,
    /// Two capital letters, or None.
    pub country: Option<String>,
    /// The website's picture of that flag.
    pub flag: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Board {
    pub updated: u64,
    /// Every key of `KEYS`, each at most 100 rows.
    pub lists: BTreeMap<String, Vec<Row>>,
}

/// None while the backend has no leaderboard (an older backend, or the public
/// list switched off).
pub async fn load() -> Result<Option<Board>> {
    let res = send(client()?.get(URL)).await?;
    if res.status().as_u16() == 404 {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(LauncherError::Message(OOPS.into()));
    }
    let body = res.json::<Envelope>().await.map_err(|_| LauncherError::Message(OOPS.into()))?;
    Ok(body.d.map(|raw| board(raw, &site_url())))
}

fn board(raw: RawBoard, site: &str) -> Board {
    let mut lists = BTreeMap::new();
    for key in KEYS {
        let rows = raw.modes.get(key).map(Vec::as_slice).unwrap_or_default();
        let rows = rows
            .iter()
            .filter(|r| r.rank >= 1 && r.rp.is_finite())
            .take(100)
            .map(|r| {
                let country = r
                    .country
                    .as_deref()
                    .filter(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_uppercase()))
                    .map(str::to_string);
                Row {
                    rank: r.rank as u32,
                    name: if r.name.trim().is_empty() { "?".into() } else { r.name.trim().to_string() },
                    rp: r.rp as i64,
                    tier: r.tier,
                    flag: country.as_ref().map(|c| format!("{site}/flags/{}.svg", c.to_lowercase())),
                    country,
                }
            })
            .collect();
        lists.insert(key.to_string(), rows);
    }
    Board { updated: raw.updated, lists }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_list_is_there_rows_are_checked_and_flags_are_the_websites() {
        let raw: Envelope = serde_json::from_str(
            r#"{"e":"NONE","c":0,"d":{"season_id":1,"updated":7,"modes":{
                "solo_tpp":[{"rank":1,"name":" Kim ","rp":494,"tier":420100032,"country":"KR"},
                            {"rank":2,"name":"","rp":10,"tier":0,"country":"../x"},
                            {"rank":0,"name":"Nobody","rp":1,"tier":0}],
                "made_up":[{"rank":1,"name":"x","rp":1,"tier":0}]}}}"#,
        )
        .unwrap();
        let b = board(raw.d.unwrap(), "https://superpeople.dev");
        assert_eq!(b.lists.len(), 8);
        assert!(!b.lists.contains_key("made_up"));
        let solo = &b.lists["solo_tpp"];
        assert_eq!(solo.len(), 2);
        assert_eq!(solo[0].name, "Kim");
        assert_eq!(solo[0].flag.as_deref(), Some("https://superpeople.dev/flags/kr.svg"));
        assert_eq!(solo[1].name, "?");
        assert_eq!(solo[1].country, None);
        assert!(b.lists["squad_fpp"].is_empty());
    }
}
