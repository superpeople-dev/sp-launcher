//! The Play tab's news feed.
//!
//! The feed is just a JSON array of `NewsItem`; this module only defines its
//! shape and how to fetch it. `starts_at`/`ends_at` are informational —
//! whether an item is currently active is decided by the frontend, so a
//! launcher upgrade is never required to change what's showing.
//!
//! Slide images on plain http are downloaded here and handed to the page as
//! `data:` URLs. Since 0.4.27 the page is served over https (Twitch's player
//! needs it), and the webview turns an http image on an https page into an
//! https request, which the game backend does not answer: every slide the
//! admin panel made (`http://<backend>:8080/banner/img/...`) went blank.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub id: String,
    pub tag: String,
    pub title: String,
    pub description: String,
    /// URL of the slide's background image.
    pub image: String,
    /// ISO-8601. Empty means "already started".
    #[serde(default)]
    pub starts_at: String,
    /// ISO-8601. Empty means "never ends".
    #[serde(default)]
    pub ends_at: String,
    pub clickable: bool,
    /// Only meaningful when `clickable` is true.
    #[serde(default)]
    pub url: Option<String>,
}

/// Where the feed lives. Fixed rather than configurable: it is this
/// server's feed, and a player pointing the launcher at some other URL only
/// ever breaks their own news panel.
pub const FEED_URL: &str = "http://64.226.112.204/launcher/news.json";

/// The admin panel takes uploads up to 2 MB; anything much bigger is not a slide.
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const IMAGE_TIMEOUT: Duration = Duration::from_secs(15);
/// Embedded images by their URL, so the feed's 10-minute refresh does not
/// download them again. A slide's image is uploaded under a new name when it
/// changes, so a URL keeps its picture.
const CACHE_MAX: usize = 32;
static EMBEDDED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Fetches and parses the feed from `url` (in practice always `FEED_URL`;
/// taking it as an argument keeps this testable against a local server). An
/// empty url yields an empty feed rather than an error.
pub async fn fetch(url: &str) -> Result<Vec<NewsItem>> {
    if url.is_empty() {
        return Ok(Vec::new());
    }
    let mut items = reqwest::get(url)
        .await?
        .error_for_status()?
        .json::<Vec<NewsItem>>()
        .await?;
    embed_images(&mut items).await;
    Ok(items)
}

/// Every plain-http slide image becomes a `data:` URL. One that cannot be
/// fetched, or is not a picture, keeps its URL (and stays blank, as before).
async fn embed_images(items: &mut [NewsItem]) {
    if !items.iter().any(|item| item.image.starts_with("http://")) {
        return;
    }
    let Ok(client) = reqwest::Client::builder().timeout(IMAGE_TIMEOUT).build() else { return };
    for item in items.iter_mut().filter(|item| item.image.starts_with("http://")) {
        let known = EMBEDDED.lock().ok().and_then(|cache| cache.as_ref().and_then(|c| c.get(&item.image).cloned()));
        if let Some(data) = known {
            item.image = data;
            continue;
        }
        if let Some(data) = embed(&client, &item.image).await {
            if let Ok(mut cache) = EMBEDDED.lock() {
                let cache = cache.get_or_insert_with(HashMap::new);
                if cache.len() >= CACHE_MAX {
                    cache.clear();
                }
                cache.insert(item.image.clone(), data.clone());
            }
            item.image = data;
        }
    }
}

/// The picture at `url` as a `data:` URL: PNG, JPEG, GIF or WebP, at most
/// `MAX_IMAGE_BYTES`. `None` for anything else.
async fn embed(client: &reqwest::Client, url: &str) -> Option<String> {
    let mut res = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let mime = res.headers().get(reqwest::header::CONTENT_TYPE)?.to_str().ok()?.split(';').next()?.trim().to_ascii_lowercase();
    if !matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp") {
        return None;
    }
    if res.content_length().is_some_and(|n| n > MAX_IMAGE_BYTES as u64) {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.ok()? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
    }
    if bytes.is_empty() {
        return None;
    }
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server that answers each request with the next (path, content type,
    /// body) whose path matches, and counts what it was asked for.
    async fn server(routes: Vec<(&'static str, &'static str, Vec<u8>)>, requests: usize) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            let mut asked = Vec::new();
            for _ in 0..requests {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap();
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                asked.push(path.clone());
                let (status, kind, body) = match routes.iter().find(|(p, _, _)| *p == path) {
                    Some((_, kind, body)) => ("200 OK", *kind, body.clone()),
                    None => ("404 Not Found", "text/plain", b"no".to_vec()),
                };
                let mut reply = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                reply.extend_from_slice(&body);
                let _ = sock.write_all(&reply).await;
            }
            asked
        });
        (base, task)
    }

    fn item(id: &str, image: &str) -> NewsItem {
        NewsItem {
            id: id.into(),
            tag: "Info".into(),
            title: "t".into(),
            description: "d".into(),
            image: image.into(),
            starts_at: String::new(),
            ends_at: String::new(),
            clickable: false,
            url: None,
        }
    }

    #[tokio::test]
    async fn http_slide_images_are_embedded_and_the_rest_left_alone() {
        let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        let (base, server) = server(
            vec![("/banner/img/a.png", "image/png", png.clone()), ("/banner/img/page.html", "text/html; charset=utf-8", b"<html>".to_vec())],
            3,
        )
        .await;
        let mut items = vec![
            item("a", &format!("{base}/banner/img/a.png")),
            item("html", &format!("{base}/banner/img/page.html")),
            item("gone", &format!("{base}/banner/img/missing.png")),
            item("https", "https://superpeople.dev/og.png"),
            item("none", ""),
        ];
        embed_images(&mut items).await;
        let asked = server.await.unwrap();
        assert_eq!(items[0].image, format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png)));
        assert!(items[1].image.ends_with("/page.html"), "not a picture: the URL stays");
        assert!(items[2].image.ends_with("/missing.png"), "not there: the URL stays");
        assert_eq!(items[3].image, "https://superpeople.dev/og.png", "https works on the page as it is");
        assert_eq!(items[4].image, "");
        assert_eq!(asked.len(), 3, "only the http images were asked for: {asked:?}");

        // The next refresh takes the picture from memory.
        let mut again = vec![item("a", &format!("{base}/banner/img/a.png"))];
        embed_images(&mut again).await;
        assert_eq!(again[0].image, items[0].image);
    }

    #[tokio::test]
    async fn the_feed_comes_back_with_its_images_embedded() {
        let jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 9, 9];
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        drop(listener);
        let feed = format!(r#"[{{"id":"w","tag":"Info","title":"Welcome","description":"d","image":"{base}/banner/img/w.jpg","clickable":true,"url":"https://discord.gg/x"}}]"#);
        let feed: &'static str = Box::leak(feed.into_boxed_str());
        let listener = tokio::net::TcpListener::bind(base.trim_start_matches("http://")).await.unwrap();
        let served = tokio::spawn(async move {
            for (kind, body) in [("application/json", feed.as_bytes().to_vec()), ("image/jpeg", jpg)] {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await.unwrap();
                let mut reply = format!("HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                reply.extend_from_slice(&body);
                let _ = sock.write_all(&reply).await;
            }
        });
        let items = fetch(&format!("{base}/launcher/news.json")).await.unwrap();
        served.await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].image.starts_with("data:image/jpeg;base64,/9j/"), "{}", &items[0].image[..40.min(items[0].image.len())]);
        assert_eq!(items[0].url.as_deref(), Some("https://discord.gg/x"));
    }
}
