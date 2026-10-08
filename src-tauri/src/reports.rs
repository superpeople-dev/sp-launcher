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
//!
//! Before a report goes, the replay of its match is uploaded and the report
//! links it (replays.rs). A report made during a match waits for the match to
//! end, so the staff get the whole of it.

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

/// The files waiting with a given name prefix, oldest first (the DLL names them
/// by the time they were made). A `.part` file is one the DLL is still writing.
/// Player reports are `report-*.json`; anti-tamper notices `tamper-*.json`.
fn pending(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix)))
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
/// has to wait. `running`: the game is still up, so a report may wait for its
/// match to end. Returns how many went.
pub async fn send_pending(session: &str, dir: &Path, running: bool) -> usize {
    let ctx = crate::replays::Ctx::live(running);
    send_all(&format!("{}/api/launcher/report", crate::auth::site_url()), session, dir, &SENDING, &ctx).await
}

async fn send_all(url: &str, session: &str, dir: &Path, busy: &AtomicBool, ctx: &crate::replays::Ctx) -> usize {
    if busy.swap(true, Ordering::AcqRel) {
        return 0;
    }
    let mut sent = 0;
    if let Ok(client) = crate::auth::client() {
        for file in pending(dir, "report-") {
            match send_one(&client, url, session, &file, ctx).await {
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

/// One anti-tamper pass at a time (same reasoning as SENDING).
static TAMPER_SENDING: AtomicBool = AtomicBool::new(false);

/// Sends the anti-tamper notices the DLL left (`tamper-*.json`) to the site's
/// tamper endpoint, as the signed-in account -- so a notice only ever concerns
/// the player who sent it. Simpler than a report: no replay, no match wait.
/// True when the site banned the player for one (`{ "banned": true }`, a
/// debugger or a known cheat): the caller closes the game at once.
pub async fn send_tamper(session: &str, dir: &Path) -> bool {
    if TAMPER_SENDING.swap(true, Ordering::AcqRel) {
        return false;
    }
    let url = format!("{}/api/launcher/tamper", crate::auth::site_url());
    let mut banned = false;
    if let Ok(client) = crate::auth::client() {
        for file in pending(dir, "tamper-") {
            match send_tamper_one(&client, &url, session, &file).await {
                (Outcome::Sent, said) => {
                    banned |= said;
                    let _ = std::fs::remove_file(&file);
                }
                (Outcome::Dropped, _) => {
                    let _ = std::fs::remove_file(&file);
                }
                (Outcome::Later, _) => break,
            }
        }
    }
    TAMPER_SENDING.store(false, Ordering::Release);
    banned
}

/// The site's answer to a notice says the player was banned for it.
fn said_banned(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|answer| answer.get("banned").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/// What became of one notice, and whether the site banned the player for it.
async fn send_tamper_one(client: &reqwest::Client, url: &str, session: &str, file: &Path) -> (Outcome, bool) {
    let Ok(meta) = std::fs::metadata(file) else { return (Outcome::Dropped, false) };
    let old = meta
        .modified()
        .ok()
        .and_then(|made| SystemTime::now().duration_since(made).ok())
        .is_some_and(|age| age > MAX_AGE);
    if old || meta.len() > MAX_BYTES {
        return (Outcome::Dropped, false);
    }
    let Ok(text) = std::fs::read_to_string(file) else { return (Outcome::Dropped, false) };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return (Outcome::Dropped, false) };
    if !value.is_object() {
        return (Outcome::Dropped, false);
    }
    let Ok(res) = client.post(url).bearer_auth(session).json(&value).send().await else {
        return (Outcome::Later, false);
    };
    match res.status().as_u16() {
        200..=299 => (Outcome::Sent, res.text().await.is_ok_and(|body| said_banned(&body))),
        400 | 413 | 422 => (Outcome::Dropped, false),
        _ => (Outcome::Later, false),
    }
}

/// When the report was made: the DLL names the file after it
/// (`report-<unix ms>-<pid>-<n>.json`), else the file's own time.
fn made_at(file: &Path, meta: &std::fs::Metadata) -> u64 {
    file.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("report-"))
        .and_then(|s| s.split('-').next())
        .and_then(|n| n.parse::<u64>().ok())
        .filter(|&n| n > 1_000_000_000_000)
        .or_else(|| meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64))
        .unwrap_or(0)
}

/// The report as it is now (its replay found or given up on), so the next
/// pass does not look again.
fn keep(file: &Path, report: &serde_json::Value) {
    let part = file.with_extension("json.part");
    if std::fs::write(&part, report.to_string()).is_ok() {
        let _ = std::fs::rename(&part, file);
    }
}

async fn send_one(client: &reqwest::Client, url: &str, session: &str, file: &Path, ctx: &crate::replays::Ctx) -> Outcome {
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
    if let (Some(fields), Some(reports)) = (report.as_object_mut(), file.parent()) {
        let had = fields.len();
        if crate::replays::attach(ctx, session, reports, fields, made_at(file, &meta)).await == crate::replays::Step::Wait {
            return Outcome::Later;
        }
        if fields.len() != had {
            keep(file, &report);
        }
    }
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

    /// No Demos folder: every report goes without a replay, asking nobody for one.
    fn no_replays() -> crate::replays::Ctx {
        crate::replays::Ctx { demos: None, logs: None, site: "http://127.0.0.1:9".into(), running: false }
    }

    /// One request as a test server saw it: the request line and headers, and the body.
    struct Seen {
        head: String,
        body: Vec<u8>,
    }

    /// A server on `port` (0: any) that answers each request with the next (status,
    /// JSON body), and hands back what it was sent, bodies whole (a zip is bigger
    /// than one read).
    async fn server_on(port: u16, replies: Vec<(u16, &'static str)>) -> (String, tokio::task::JoinHandle<Vec<Seen>>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut got = Vec::new();
                let mut buf = vec![0u8; 65536];
                let (head, length) = loop {
                    let n = sock.read(&mut buf).await.unwrap();
                    got.extend_from_slice(&buf[..n]);
                    if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&got[..end]).to_string();
                        let length = head
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap()))
                            .unwrap_or(0);
                        got.drain(..end + 4);
                        break (head, length);
                    }
                };
                while got.len() < length {
                    let n = sock.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                seen.push(Seen { head, body: got });
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = sock.write_all(reply.as_bytes()).await;
            }
            seen
        });
        (base, task)
    }

    /// The game's recording of a match that ran from `start` for 20 minutes, over.
    fn match_recording(demos: &Path, name: &str, start: u64, live: bool) {
        let dir = demos.join(name);
        std::fs::create_dir_all(dir.join("checkpoints")).unwrap();
        std::fs::write(dir.join("MK3D.demo"), vec![3u8; 100_000]).unwrap();
        std::fs::write(dir.join("checkpoints").join("checkpoint0"), b"c").unwrap();
        let json = format!(r#"{{"LengthInMS": 1200000, "Timestamp": {start}, "bIsLive": {live}, "RecordUserId": "{ME}"}}"#);
        let mut info = ((json.len() + 1) as i32).to_le_bytes().to_vec();
        info.extend_from_slice(json.as_bytes());
        info.push(0);
        std::fs::write(dir.join("MK3D.replayinfo"), info).unwrap();
        if !live {
            std::fs::write(dir.join("MK3D.final"), b"").unwrap();
        }
    }

    const ME: &str = "f5a441e76deab1b92db6074e0d6ab07c";

    #[tokio::test]
    async fn the_replay_of_the_reported_match_goes_up_once_and_each_report_links_it() {
        let dir = tempfile::tempdir().unwrap();
        let demos = tempfile::tempdir().unwrap();
        let start = 1_790_900_000_000u64;
        match_recording(demos.path(), "kapi_2026-10-01_20-25", start, false);
        let body = format!(r#"{{"v":1,"reason":1,"replay":"{ME}_r__103_2026-10-02_00_37_03.7z"}}"#);
        report(dir.path(), &format!("report-{}-1-1.json", start + 5 * 60_000), &body);
        report(dir.path(), &format!("report-{}-1-2.json", start + 9 * 60_000), &body);
        // The site's answer names an upload link on the same stand-in server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let started: &'static str = Box::leak(
            format!(r#"{{"id":"{0}","upload_url":"http://127.0.0.1:{port}/media/replays/{0}/kapi.zip?sig=1","url":"https://superpeople.dev/replays/{0}","max_bytes":67108864}}"#, "ab".repeat(16))
                .into_boxed_str(),
        );
        let (base, server) = server_on(port, vec![(200, started), (200, ""), (204, ""), (204, "")]).await;
        let ctx = crate::replays::Ctx { demos: Some(demos.path().to_path_buf()), logs: None, site: base.clone(), running: false };
        let url = format!("{base}/api/launcher/report");
        assert_eq!(send_all(&url, "session-token", dir.path(), &AtomicBool::new(false), &ctx).await, 2);
        let seen = server.await.unwrap();
        let ask = String::from_utf8_lossy(&seen[0].body).to_string();
        assert!(seen[0].head.starts_with("POST /api/launcher/replays ") && seen[0].head.to_ascii_lowercase().contains("authorization: bearer session-token"), "{}", seen[0].head);
        assert!(ask.contains(r#""name":"kapi_2026-10-01_20-25""#) && ask.contains(r#""bytes":"#), "{ask}");
        let zip_len = seen[1].body.len();
        assert!(seen[1].head.starts_with(&format!("PUT /media/replays/{}/kapi.zip?sig=1 ", "ab".repeat(16))), "{}", seen[1].head);
        assert!(seen[1].head.to_ascii_lowercase().contains("content-type: application/zip"));
        assert!(seen[1].body.starts_with(b"PK") && zip_len < 100_000, "the recording, zipped, straight to storage");
        assert!(ask.contains(&format!(r#""bytes":{zip_len}"#)), "the site is told the zip's size: {ask}");
        for report in &seen[2..] {
            let text = String::from_utf8_lossy(&report.body);
            assert!(report.head.starts_with("POST /api/launcher/report "));
            assert!(text.contains(&format!(r#""replay_url":"https://superpeople.dev/replays/{}""#, "ab".repeat(16))), "{text}");
            assert!(text.contains(&format!(r#""replay_bytes":{zip_len}"#)) && text.contains(r#""replay_match":"kapi_2026-10-01_20-25""#), "{text}");
        }
        assert_eq!(seen.len(), 4, "the second report of that match did not upload it again");
    }

    #[tokio::test]
    async fn a_report_made_during_a_match_waits_for_it_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let demos = tempfile::tempdir().unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        match_recording(demos.path(), "kapi_now", now - 5 * 60_000, true);
        let file = report(dir.path(), &format!("report-{}-1-1.json", now - 60_000), r#"{"v":1}"#);
        let ctx = crate::replays::Ctx { demos: Some(demos.path().to_path_buf()), logs: None, site: "http://127.0.0.1:9".into(), running: true };
        // Nobody listens: had it asked anyone, it would have been told nothing.
        assert_eq!(send_all("http://127.0.0.1:9/x", "s", dir.path(), &AtomicBool::new(false), &ctx).await, 0);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), r#"{"v":1}"#, "untouched until the match is over");
    }

    #[tokio::test]
    async fn reports_go_oldest_first_with_the_session_and_leave_once_taken() {
        let dir = tempfile::tempdir().unwrap();
        let second = report(dir.path(), "report-1700000000002-1-2.json", r#"{"v":1,"reason":2}"#);
        let first = report(dir.path(), "report-1700000000001-1-1.json", r#"{"v":1,"reason":1}"#);
        let writing = report(dir.path(), "report-1700000000003-1-3.part", r#"{"v":1"#);
        let (url, server) = site(vec![204, 200]).await;
        assert_eq!(send_all(&url, "session-token", dir.path(), &AtomicBool::new(false), &no_replays()).await, 2);
        let seen = server.await.unwrap();
        assert!(seen[0].contains(r#""reason":1"#) && seen[1].contains(r#""reason":2"#), "{seen:?}");
        assert!(seen.iter().all(|r| r.contains("authorization: Bearer session-token") || r.contains("Authorization: Bearer session-token")));
        assert!(seen[0].contains(r#""version":""#), "the launcher's version goes along");
        assert!(seen[0].contains(r#""replay_note":"missing""#), "no recording of the match: the report says so");
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
        assert_eq!(send_all(&url, "s", dir.path(), &AtomicBool::new(false), &no_replays()).await, 0);
        assert_eq!(server.await.unwrap().len(), 2);
        assert!(!refused.exists());
        assert!(waiting.exists() && after.exists());

        // Signed out (401) or the site down: still kept.
        let (url, server) = site(vec![401]).await;
        assert_eq!(send_all(&url, "s", dir.path(), &AtomicBool::new(false), &no_replays()).await, 0);
        server.await.unwrap();
        assert!(waiting.exists() && after.exists());
    }

    #[tokio::test]
    async fn no_site_at_all_keeps_every_report() {
        let dir = tempfile::tempdir().unwrap();
        let kept = report(dir.path(), "report-1-1-1.json", r#"{"v":1}"#);
        // Nothing listens on this port once the listener is gone.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(send_all(&format!("http://127.0.0.1:{port}/x"), "s", dir.path(), &AtomicBool::new(false), &no_replays()).await, 0);
        assert!(kept.exists());
    }

    #[tokio::test]
    async fn what_is_not_a_report_is_thrown_away_without_asking_the_site() {
        let dir = tempfile::tempdir().unwrap();
        let broken = report(dir.path(), "report-1-1-1.json", "{not json");
        let list = report(dir.path(), "report-2-1-2.json", "[1,2]");
        let huge = report(dir.path(), "report-3-1-3.json", &format!(r#"{{"v":"{}"}}"#, "x".repeat(40_000)));
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(send_all(&format!("http://127.0.0.1:{port}/x"), "s", dir.path(), &AtomicBool::new(false), &no_replays()).await, 0);
        assert!(!broken.exists() && !list.exists() && !huge.exists());
    }

    #[test]
    fn the_folder_is_made_in_the_launchers_own() {
        let config = tempfile::tempdir().unwrap();
        let dir = prepare(config.path()).expect("folder");
        assert_eq!(dir, config.path().join("reports"));
        assert!(dir.is_dir());
        assert!(pending(&dir, "report-").is_empty());
        assert!(pending(&dir, "tamper-").is_empty());
    }

    #[test]
    fn a_tamper_answer_says_banned_only_when_it_does() {
        assert!(said_banned(r#"{"banned":true}"#));
        assert!(!said_banned(r#"{"banned":false}"#));
        assert!(!said_banned(""), "a 204 has no body: not banned");
        assert!(!said_banned("not json"));
        assert!(!said_banned(r#"{"banned":"yes"}"#));
    }

    #[test]
    fn pending_keeps_reports_and_tamper_apart() {
        let config = tempfile::tempdir().unwrap();
        let dir = prepare(config.path()).expect("folder");
        std::fs::write(dir.join("report-1-2-1.json"), "{}").unwrap();
        std::fs::write(dir.join("tamper-1-2-1.json"), "{}").unwrap();
        std::fs::write(dir.join("report-2-2-1.json.part"), "{}").unwrap();
        let reports = pending(&dir, "report-");
        let tamper = pending(&dir, "tamper-");
        assert_eq!(reports.len(), 1, "only the finished report, not the .part");
        assert!(reports[0].file_name().unwrap().to_str().unwrap().starts_with("report-"));
        assert_eq!(tamper.len(), 1);
        assert!(tamper[0].file_name().unwrap().to_str().unwrap().starts_with("tamper-"));
    }
}
