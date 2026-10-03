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
    /// The channel's picture, its bio, "partner" or "affiliate" (else empty) and when it joined Twitch.
    /// A website from before them sends none.
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub badge: String,
    #[serde(default)]
    pub since: String,
    /// The stream's tags and whether it is marked for mature audiences.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub mature: bool,
    /// The channel's most watched SUPER PEOPLE clips of the last 30 days.
    #[serde(default)]
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Clip {
    pub title: String,
    /// Its page on Twitch (clips.twitch.tv), opened in the browser.
    pub url: String,
    #[serde(default)]
    pub thumbnail: String,
    #[serde(default)]
    pub views: u64,
    #[serde(default)]
    pub seconds: u64,
    #[serde(default)]
    pub created_at: String,
}

const BIO_MAX: usize = 300;
const TAGS_MAX: usize = 10;
const TAG_MAX: usize = 25;
const CLIPS_MAX: usize = 4;
const CLIP_TITLE_MAX: usize = 100;

/// Pictures come from Twitch's CDN only.
fn twitch_image(url: &str) -> bool {
    url.starts_with("https://static-cdn.jtvnw.net/") || url.starts_with("https://clips-media-assets2.twitch.tv/")
}

fn clipped(text: &str, max: usize) -> String {
    text.trim().chars().take(max).collect()
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
/// characters (it goes into a twitch.tv address), previews and pictures from
/// Twitch's CDN, clips on Twitch's own pages, Twitch's own category page, and
/// text and lists of a sensible length.
fn cleaned(mut got: Streams) -> Streams {
    if let Some(streams) = got.streams.as_mut() {
        streams.retain(|s| !s.login.is_empty() && s.login.len() <= 25 && s.login.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        for s in streams.iter_mut() {
            if !s.thumbnail.starts_with("https://static-cdn.jtvnw.net/") {
                s.thumbnail.clear();
            }
            if !twitch_image(&s.avatar) {
                s.avatar.clear();
            }
            s.bio = clipped(&s.bio, BIO_MAX);
            if s.badge != "partner" && s.badge != "affiliate" {
                s.badge.clear();
            }
            s.tags = s.tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && t.chars().count() <= TAG_MAX).take(TAGS_MAX).collect();
            s.clips.retain(|c| c.url.starts_with("https://clips.twitch.tv/") || c.url.starts_with("https://www.twitch.tv/"));
            s.clips.truncate(CLIPS_MAX);
            for c in s.clips.iter_mut() {
                c.title = clipped(&c.title, CLIP_TITLE_MAX);
                if !twitch_image(&c.thumbnail) {
                    c.thumbnail.clear();
                }
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

    #[test]
    fn channel_details_tags_and_clips_are_checked_too() {
        let long_bio = "b".repeat(400);
        let raw = format!(
            r#"{{"configured":true,"category":"https://www.twitch.tv/directory/category/super-people","streams":[
            {{"login":"one","name":"One","title":"t","viewers":4,"startedAt":"2026-10-01T10:00:00Z",
              "avatar":"https://static-cdn.jtvnw.net/jtv_user_pictures/one-300x300.png","bio":"{long_bio}","badge":"partner","since":"2019-05-01T00:00:00Z",
              "tags":["Portuguese","  ","{}","FPS"],"mature":true,
              "clips":[
                {{"title":"ace","url":"https://clips.twitch.tv/Ace","thumbnail":"https://static-cdn.jtvnw.net/c/a.jpg","views":50,"seconds":30,"createdAt":"2026-09-29T00:00:00Z"}},
                {{"title":"phish","url":"https://evil.example/clip","thumbnail":"https://static-cdn.jtvnw.net/c/b.jpg","views":9,"seconds":5,"createdAt":"2026-09-29T00:00:00Z"}},
                {{"title":"odd picture","url":"https://www.twitch.tv/one/clip/Odd","thumbnail":"https://evil.example/x.jpg","views":1,"seconds":5,"createdAt":"2026-09-29T00:00:00Z"}}
              ]}},
            {{"login":"two","name":"Two","title":"t","viewers":1,"startedAt":"2026-10-01T10:00:00Z","avatar":"http://elsewhere/p.png","badge":"staff"}}
        ]}}"#,
            "x".repeat(30)
        );
        let got = cleaned(serde_json::from_str(&raw).unwrap());
        let streams = got.streams.unwrap();
        let one = &streams[0];
        assert!(one.avatar.starts_with("https://static-cdn.jtvnw.net/"));
        assert_eq!(one.bio.chars().count(), BIO_MAX);
        assert_eq!(one.badge, "partner");
        assert_eq!(one.tags, ["Portuguese", "FPS"]);
        assert!(one.mature);
        assert_eq!(one.clips.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["ace", "odd picture"]);
        assert!(one.clips[1].thumbnail.is_empty());
        let two = &streams[1];
        assert!(two.avatar.is_empty() && two.badge.is_empty() && two.tags.is_empty() && two.clips.is_empty() && two.bio.is_empty());
    }
}
