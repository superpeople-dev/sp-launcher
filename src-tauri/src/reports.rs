//! The game's Report button, sent to the staff.
//!
//! The game sent its reports to the original publisher's AWS services, which
//! are gone, so a report reached nobody. The client fixes DLL (sp-native
//! client-fixes/src/player_reports.cpp) now writes each report as a small JSON
//! file into the folder named here in `SP_REPORT_DIR`. While the game runs this
//! sends them to superpeople.dev with the player's own session (sp-website
//! app/api/launcher/report), and the site posts them to the staff's Discord
//! channel. Who reported is whoever is signed in to the launcher, never a name
//! in the file.
//!
//! A report the site read and refused is deleted; one it could not take now
//! (offline, signed out, too many in an hour) waits for the next pass. After a
//! week the game itself no longer takes a report, so neither do we.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// Where the DLL reads the folder from.
pub const ENV: &str = "SP_REPORT_DIR";
/// How often the folder is looked at while the game runs.
pub const EVERY: Duration = Duration::from_secs(5);
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// A report is well under 4 KB; anything much bigger is not one.
const MAX_BYTES: u64 = 32 * 1024;

/// One pass at a time: the pass after a game closes can meet the first one of
/// the next game, and a report must not go twice.
static SENDING: AtomicBool = AtomicBool::new(false);

/// The folder, created if needed. `None` when it cannot be: the game then
/// starts without it and the Report button does what it always did.
pub fn prepare(config_dir: &Path) -> Option<PathBuf> {
    let dir = config_dir.join("reports");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The reports waiting, oldest first (the DLL names them by the time they were
/// made). A `.part` file is one the DLL is still writing.
fn pending(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Sent,
    Dropped,
    Later,
}

/// Sends every waiting report, oldest first, and stops at the first one that
/// has to wait. Returns how many went.
pub async fn send_pending(session: &str, dir: &Path) -> usize {
    send_all(&format!("{}/api/launcher/report", crate::auth::site_url()), session, dir, &SENDING).await
}

async fn send_all(url: &str, session: &str, dir: &Path, busy: &AtomicBool) -> usize {
    if busy.swap(true, Ordering::AcqRel) {
        return 0;
    }
    let mut sent = 0;
    if let Ok(client) = crate::auth::client() {
        for file in pending(dir) {
            match send_one(&client, url, session, &file).await {
                Outcome::Sent => {
                    sent += 1;
                    let _ = std::fs::remove_file(&file);
                }
                Outcome::Dropped => {
                    let _ = std::fs::remove_file(&file);
                }
                Outcome::Later => break,
            }
        }
    }
    busy.store(false, Ordering::Release);
    sent
}

async fn send_one(client: &reqwest::Client, url: &str, session: &str, file: &Path) -> Outcome {
    let Ok(meta) = std::fs::metadata(file) else { return Outcome::Dropped };
    let old = meta
        .modified()
        .ok()
        .and_then(|made| SystemTime::now().duration_since(made).ok())
        .is_some_and(|age| age > MAX_AGE);
    if old || meta.len() > MAX_BYTES {
        return Outcome::Dropped;
    }
    let Ok(text) = std::fs::read_to_string(file) else { return Outcome::Dropped };
    let Ok(report) = serde_json::from_str::<serde_json::Value>(&text) else { return Outcome::Dropped };
    if !report.is_object() {
        return Outcome::Dropped;
    }
    let mut report = report;
    if let Some(fields) = report.as_object_mut() {
        fields.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    }
    let Ok(res) = client.post(url).bearer_auth(session).json(&report).send().await else {
        return Outcome::Later;
    };
    match res.status().as_u16() {
        200..=299 => Outcome::Sent,
        // The site read it and refused it: sending it again changes nothing.
        400 | 413 | 422 => Outcome::Dropped,
        // Signed out, too many this hour, the site down or not updated yet:
        // the same report goes through later.
        _ => Outcome::Later,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A site that answers each request with the next status, and hands back
    /// what it was sent.
    async fn site(statuses: Vec<u16>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://127.0.0.1:{}/api/launcher/report", listener.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let mut seen = Vec::new();
            for status in statuses {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let mut got = 0;
                // Headers, then the body Content-Length promises.
                loop {
                    let n = sock.read(&mut buf[got..]).await.unwrap();
                    got += n;
                    let text = String::from_utf8_lossy(&buf[..got]).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap()))
                            .unwrap_or(0);
                        if got >= end + 4 + length || n == 0 {
                            seen.push(text);
                            break;
                        }
                    }
                }
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                let _ = sock.write_all(reply.as_bytes()).await;
            }
            seen
        });
        (url, server)
    }

    fn report(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[tokio::test]
    async fn reports_go_oldest_first_with_the_session_and_leave_once_taken() {
        let dir = tempfile::tempdir().unwrap();
        let second = report(dir.path(), "report-1700000000002-1-2.json", r#"{"v":1,"reason":2}"#);
        let first = report(dir.path(), "report-1700000000001-1-1.json", r#"{"v":1,"reason":1}"#);
        let writing = report(dir.path(), "report-1700000000003-1-3.part", r#"{"v":1"#);
        let (url, server) = site(vec![204, 200]).await;
        assert_eq!(send_all(&url, "session-token", dir.path(), &AtomicBool::new(false)).await, 2);
        let seen = server.await.unwrap();
        assert!(seen[0].contains(r#""reason":1"#) && seen[1].contains(r#""reason":2"#), "{seen:?}");
        assert!(seen.iter().all(|r| r.contains("authorization: Bearer session-token") || r.contains("Authorization: Bearer session-token")));
        assert!(seen[0].contains(r#""version":""#), "the launcher's version goes along");
        assert!(!first.exists() && !second.exists());
        assert!(writing.exists(), "a report still being written is left alone");
    }

    #[tokio::test]
    async fn a_report_waits_when_the_site_cannot_take_it_and_goes_when_refused() {
        let dir = tempfile::tempdir().unwrap();
        let refused = report(dir.path(), "report-1-1-1.json", r#"{"v":1}"#);
        let waiting = report(dir.path(), "report-2-1-2.json", r#"{"v":1}"#);
        let after = report(dir.path(), "report-3-1-3.json", r#"{"v":1}"#);
        // 400: refused, deleted. 429: too many this hour, kept, and the pass stops.
        let (url, server) = site(vec![400, 429]).await;
        assert_eq!(send_all(&url, "s", dir.path(), &AtomicBool::new(false)).await, 0);
        assert_eq!(server.await.unwrap().len(), 2);
        assert!(!refused.exists());
        assert!(waiting.exists() && after.exists());

        // Signed out (401) or the site down: still kept.
        let (url, server) = site(vec![401]).await;
        assert_eq!(send_all(&url, "s", dir.path(), &AtomicBool::new(false)).await, 0);
        server.await.unwrap();
        assert!(waiting.exists() && after.exists());
    }

    #[tokio::test]
    async fn no_site_at_all_keeps_every_report() {
        let dir = tempfile::tempdir().unwrap();
        let kept = report(dir.path(), "report-1-1-1.json", r#"{"v":1}"#);
        // Nothing listens on this port once the listener is gone.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(send_all(&format!("http://127.0.0.1:{port}/x"), "s", dir.path(), &AtomicBool::new(false)).await, 0);
        assert!(kept.exists());
    }

    #[tokio::test]
    async fn what_is_not_a_report_is_thrown_away_without_asking_the_site() {
        let dir = tempfile::tempdir().unwrap();
        let broken = report(dir.path(), "report-1-1-1.json", "{not json");
        let list = report(dir.path(), "report-2-1-2.json", "[1,2]");
        let huge = report(dir.path(), "report-3-1-3.json", &format!(r#"{{"v":"{}"}}"#, "x".repeat(40_000)));
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(send_all(&format!("http://127.0.0.1:{port}/x"), "s", dir.path(), &AtomicBool::new(false)).await, 0);
        assert!(!broken.exists() && !list.exists() && !huge.exists());
    }

    #[test]
    fn the_folder_is_made_in_the_launchers_own() {
        let config = tempfile::tempdir().unwrap();
        let dir = prepare(config.path()).expect("folder");
        assert_eq!(dir, config.path().join("reports"));
        assert!(dir.is_dir());
        assert!(pending(&dir).is_empty());
    }
}
