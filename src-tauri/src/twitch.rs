//! The Twitch tab: who is live in Twitch's SUPER PEOPLE category. The website
//! asks Twitch (sp-website app/api/launcher/streams, which holds the Twitch
//! app's keys) and this asks the website: the webview's CSP allows no outside
//! hosts, and no key belongs in the launcher. The previews are plain https
//! images, which the CSP allows.

use serde::{Deserialize, Serialize};

use crate::auth::{client, send, site_url, OOPS};
use crate::error::{LauncherError, Result};

/// Twitch's page for the category, when the website does not say.
pub const CATEGORY_URL: &str = "https://www.twitch.tv/directory/category/super-people";

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    /// The channel's login: its page is twitch.tv/<login>.
    pub login: String,
    pub name: String,
    pub title: String,
    pub viewers: u64,
    /// A 440x248 preview, or empty.
    #[serde(default)]
    pub thumbnail: String,
    pub started_at: String,
    #[serde(default)]
    pub language: String,
}

/// configured: false until the website has Twitch's keys. streams: None when
/// Twitch could not be reached.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Streams {
    pub configured: bool,
    pub streams: Option<Vec<Stream>>,
    pub category: String,
}

pub async fn live() -> Result<Streams> {
    let url = format!("{}/api/launcher/streams", site_url());
    let res = send(client()?.get(url)).await?;
    // A website from before the tab.
    if res.status().as_u16() == 404 {
        return Ok(Streams { configured: false, streams: None, category: CATEGORY_URL.into() });
    }
    if !res.status().is_success() {
        return Err(LauncherError::Message(OOPS.into()));
    }
    let got = res.json::<Streams>().await.map_err(|_| LauncherError::Message(OOPS.into()))?;
    Ok(cleaned(got))
}

/// Only what is safe to open and show: a channel login of Twitch's own
/// characters (it goes into a twitch.tv address), a preview from Twitch's CDN,
/// and Twitch's own category page.
fn cleaned(mut got: Streams) -> Streams {
    if let Some(streams) = got.streams.as_mut() {
        streams.retain(|s| !s.login.is_empty() && s.login.len() <= 25 && s.login.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        for s in streams.iter_mut() {
            if !s.thumbnail.starts_with("https://static-cdn.jtvnw.net/") {
                s.thumbnail.clear();
            }
        }
    }
    if !got.category.starts_with("https://www.twitch.tv/") {
        got.category = CATEGORY_URL.into();
    }
    got
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_twitch_logins_previews_and_pages_get_through() {
        let raw = r#"{"configured":true,"category":"https://evil.example/","streams":[
            {"login":"good_one","name":"Good","title":"t","viewers":4,"thumbnail":"https://static-cdn.jtvnw.net/x-440x248.jpg","startedAt":"2026-10-01T10:00:00Z","language":"en"},
            {"login":"../../x","name":"Bad","title":"t","viewers":9,"thumbnail":"https://static-cdn.jtvnw.net/y.jpg","startedAt":"2026-10-01T10:00:00Z","language":"en"},
            {"login":"odd","name":"Odd","title":"t","viewers":1,"thumbnail":"http://elsewhere/z.jpg","startedAt":"2026-10-01T10:00:00Z"}
        ]}"#;
        let got = cleaned(serde_json::from_str(raw).unwrap());
        let streams = got.streams.unwrap();
        assert_eq!(streams.iter().map(|s| s.login.as_str()).collect::<Vec<_>>(), ["good_one", "odd"]);
        assert!(streams[0].thumbnail.starts_with("https://static-cdn.jtvnw.net/"));
        assert!(streams[1].thumbnail.is_empty());
        assert_eq!(got.category, CATEGORY_URL);
        let off: Streams = serde_json::from_str(r#"{"configured":false,"streams":null,"category":"https://www.twitch.tv/directory/category/super-people"}"#).unwrap();
        assert!(!off.configured && off.streams.is_none());
    }
}
