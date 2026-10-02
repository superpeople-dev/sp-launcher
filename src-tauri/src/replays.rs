//! The replay of a reported match, for the staff.
//!
//! The game records every match it plays, into
//! `%LOCALAPPDATA%\BravoHotelGame\Saved\Demos\<name>_<date>_<time>\`: the
//! recording (`*.demo`), its header and checkpoints, `*.final` once it is over,
//! and `*.replayinfo`, which says when it started, how long it ran, whether it
//! is still being recorded and whose it is. A report names a `.7z` the game only
//! ever made for the original publisher's S3 upload, which has no address now,
//! so that file never exists. Instead, before a report goes out (reports.rs),
//! the recording of the match it was made in is zipped as its folder (the staff
//! unzip it into their own Demos folder and open it from the game's Replay
//! menu) and uploaded to the game backend with a game pass (sp-backend
//! lib/replays.js). The report then carries the staff's link to it.
//!
//! A report made during a match waits for that match to end, since the
//! recording is only whole then, but not longer than [`WAIT`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// What the backend takes at most (sp-backend lib/replays.js MAX_BYTES). A
/// whole match zips to 2-15 MB.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// More than this unzipped is no recording of one match: not even zipped.
const MAX_RAW: u64 = 1024 * 1024 * 1024;
/// How far apart the report's clock and the recording's may be.
const SLACK_MS: u64 = 90_000;
/// How long a report waits for its match to end, or for the backend to take
/// the replay, while the game runs. Once the game is closed it waits no more.
pub const WAIT: Duration = Duration::from_secs(45 * 60);
/// An upload of 15 MB on a slow line takes minutes, not the usual seconds.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Uploaded recordings, so a second report of the same match links the same
/// replay instead of sending it again. In the reports folder, and not a
/// `.json`: that would read as a report.
const SENT_FILE: &str = "replays.sent";

/// The game's Demos folder.
pub fn demos_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(Path::new(&base).join("BravoHotelGame").join("Saved").join("Demos"))
}

/// Where a replay goes and what a game pass is asked for with.
pub struct Ctx {
    /// The game's Demos folder; `None` when there is none to look in.
    pub demos: Option<PathBuf>,
    /// The website, for the game pass (`/api/launcher/pass`).
    pub site: String,
    /// The game backend's launcher API (`/replays`).
    pub backend: String,
    /// Whether the game is still running: a report may then wait for its match.
    pub running: bool,
}

impl Ctx {
    pub fn live(running: bool) -> Self {
        Ctx { demos: demos_dir(), site: crate::auth::site_url(), backend: crate::auth::AUTH_BASE_URL.into(), running }
    }
}

fn ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

// ----------------------------------------------------------- recordings ---

/// One recording, as its folder and `*.replayinfo` describe it.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub dir: PathBuf,
    /// When it started (ms since 1970, UTC) and how long it ran, from `*.replayinfo`.
    start: Option<u64>,
    length: u64,
    /// The game account it was recorded on (32 hex), when known.
    user: String,
    /// Over: the game wrote `*.final` and no longer calls it live.
    finished: bool,
    /// When anything in it was last written (ms).
    touched: u64,
}

#[derive(serde::Deserialize, Default)]
struct Info {
    #[serde(default, rename = "Timestamp")]
    timestamp: u64,
    #[serde(default, rename = "LengthInMS")]
    length: u64,
    #[serde(default, rename = "bIsLive")]
    live: bool,
    #[serde(default, rename = "RecordUserId")]
    user: String,
}

/// `*.replayinfo`: the engine's string (a 4-byte length, then UTF-8, or
/// UTF-16 when the length is negative) holding a JSON object.
fn read_info(path: &Path) -> Option<Info> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 6 || bytes.len() > 256 * 1024 {
        return None;
    }
    let length = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let text = if length < 0 {
        let units: Vec<u16> = bytes[4..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&bytes[4..]).into_owned()
    };
    let (open, close) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(open..=close)?).ok()
}

fn read(dir: &Path) -> Option<Recording> {
    let mut info = None;
    let (mut final_mark, mut demo, mut touched) = (false, false, 0);
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if let Ok(meta) = entry.metadata() {
            touched = touched.max(meta.modified().map(ms).unwrap_or(0));
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("replayinfo") => info = read_info(&path),
            Some("final") => final_mark = true,
            Some("demo") => demo = true,
            _ => {}
        }
    }
    if !demo {
        return None;
    }
    let info = info.unwrap_or_default();
    Some(Recording {
        dir: dir.to_path_buf(),
        start: (info.timestamp > 0).then_some(info.timestamp),
        length: info.length,
        user: info.user,
        finished: final_mark && !info.live,
        touched,
    })
}

/// Every recording in the Demos folder.
pub fn recordings(demos: &Path) -> Vec<Recording> {
    let Ok(entries) = std::fs::read_dir(demos) else { return Vec::new() };
    entries.flatten().filter(|e| e.path().is_dir()).filter_map(|e| read(&e.path())).collect()
}

/// The recording a report made at `at` (ms) belongs to: a finished one that
/// ran then, or else one still being written since then. `user` is the game
/// account the report names; another account's recordings on the same PC are
/// left out.
pub fn pick(list: &[Recording], at: u64, user: &str) -> Option<Recording> {
    let mine = |r: &&Recording| user.is_empty() || r.user.is_empty() || r.user.eq_ignore_ascii_case(user);
    let ran_then = |r: &&Recording| r.start.is_some_and(|s| s <= at + SLACK_MS && at <= s + r.length + SLACK_MS);
    if let Some(found) = list.iter().filter(mine).filter(|r| r.finished).filter(ran_then).max_by_key(|r| r.start) {
        return Some(found.clone());
    }
    list.iter()
        .filter(mine)
        .filter(|r| !r.finished && r.touched + SLACK_MS >= at && r.start.is_none_or(|s| s <= at + SLACK_MS))
        .max_by_key(|r| r.touched)
        .cloned()
}

/// The game account a report names: the start of its replay's name
/// (`<account>_r_..._<time>.7z`), or "" when it names none.
fn user_of(replay: &str) -> &str {
    match replay.split_once("_r_") {
        Some((user, _)) if user.len() == 32 && user.bytes().all(|b| b.is_ascii_hexdigit()) => user,
        _ => "",
    }
}

// --------------------------------------------------------------- zip ---

#[derive(Debug, PartialEq)]
pub enum ZipProblem {
    TooBig,
    Unreadable,
}

fn files_in(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files_in(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// The recording's folder as a zip, the folder itself at its top.
pub fn zip(dir: &Path) -> Result<Vec<u8>, ZipProblem> {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or(ZipProblem::Unreadable)?;
    let mut files = Vec::new();
    files_in(dir, &mut files).map_err(|_| ZipProblem::Unreadable)?;
    let raw: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
    if raw > MAX_RAW {
        return Err(ZipProblem::TooBig);
    }
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for file in files {
        let rel = file.strip_prefix(dir).map_err(|_| ZipProblem::Unreadable)?;
        let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        out.start_file(format!("{name}/{}", rel.join("/")), options).map_err(|_| ZipProblem::Unreadable)?;
        let mut input = std::fs::File::open(&file).map_err(|_| ZipProblem::Unreadable)?;
        std::io::copy(&mut input, &mut out).map_err(|_| ZipProblem::Unreadable)?;
    }
    let mut cursor = out.finish().map_err(|_| ZipProblem::Unreadable)?;
    cursor.flush().map_err(|_| ZipProblem::Unreadable)?;
    let bytes = cursor.into_inner();
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ZipProblem::TooBig);
    }
    Ok(bytes)
}

// ------------------------------------------------------------ upload ---

#[derive(Debug, PartialEq)]
enum Upload {
    Stored { url: String, bytes: u64 },
    TooBig,
    /// The backend read it and will not take it, or has no place for it.
    Refused,
    /// Offline, signed out, the backend busy: the same replay may go later.
    Later,
}

#[derive(serde::Deserialize)]
struct PassOk {
    pass: String,
}

#[derive(serde::Deserialize)]
struct Stored {
    url: String,
    #[serde(default)]
    bytes: u64,
}

/// What the recording is called in a header: its folder's name, plain.
fn header_name(dir: &Path) -> String {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).take(80).collect()
}

async fn upload(ctx: &Ctx, session: &str, dir: &Path, body: Vec<u8>) -> Upload {
    let Ok(client) = crate::auth::client() else { return Upload::Later };
    // A game pass of the player's own, as for Play: the backend files the replay under them.
    let pass = match client.post(format!("{}/api/launcher/pass", ctx.site)).bearer_auth(session).send().await {
        Ok(res) if res.status().is_success() => match res.json::<PassOk>().await {
            Ok(ok) => ok.pass,
            Err(_) => return Upload::Later,
        },
        // Banned, or the terms changed: no pass to send it with, and the report goes without it.
        Ok(res) if res.status().as_u16() == 403 => return Upload::Refused,
        _ => return Upload::Later,
    };
    let res = client
        .post(format!("{}/replays", ctx.backend))
        .timeout(UPLOAD_TIMEOUT)
        .header("x-sp-pass", pass)
        .header("x-sp-replay", header_name(dir))
        .header(reqwest::header::CONTENT_TYPE, "application/zip")
        .body(body)
        .send()
        .await;
    let Ok(res) = res else { return Upload::Later };
    match res.status().as_u16() {
        200..=299 => match res.json::<Stored>().await {
            Ok(stored) if stored.url.starts_with("https://") => Upload::Stored { url: stored.url, bytes: stored.bytes },
            _ => Upload::Refused,
        },
        413 => Upload::TooBig,
        // Not a zip, or a backend without replays yet.
        400 | 404 | 411 => Upload::Refused,
        _ => Upload::Later,
    }
}

// --------------------------------------------------- uploaded before ---

fn sent_key(rec: &Recording) -> String {
    format!("{}|{}", header_name(&rec.dir), rec.start.unwrap_or(rec.touched))
}

fn sent(reports: &Path) -> Map<String, Value> {
    std::fs::read_to_string(reports.join(SENT_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn remember(reports: &Path, rec: &Recording, url: &str, bytes: u64) {
    let now = ms(SystemTime::now());
    let mut list = sent(reports);
    // A week on, no report of that match is still waiting.
    list.retain(|_, v| v.get("at").and_then(Value::as_u64).is_some_and(|at| now.saturating_sub(at) < 7 * 86_400_000));
    list.insert(sent_key(rec), serde_json::json!({ "url": url, "bytes": bytes, "at": now }));
    let _ = std::fs::write(reports.join(SENT_FILE), Value::Object(list).to_string());
}

// ------------------------------------------------------------ report ---

/// What became of the replay step for one report.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// The report now says where its replay is, or why it has none.
    Done,
    /// Not yet: its match is still being played, or the upload has to wait.
    Wait,
}

fn note(report: &mut Map<String, Value>, why: &str) -> Step {
    report.insert("replay_note".into(), why.into());
    Step::Done
}

/// Adds the replay to `report` (made at `at`, ms): `replay_url`, `replay_bytes`
/// and `replay_match` (the recording's folder), or `replay_note` ("missing",
/// "too_big", "failed") when it goes without one. A report that has either
/// already is left as it is. `reports` is the reports folder, which also keeps
/// what was uploaded.
pub async fn attach(ctx: &Ctx, session: &str, reports: &Path, report: &mut Map<String, Value>, at: u64) -> Step {
    if report.contains_key("replay_url") || report.contains_key("replay_note") {
        return Step::Done;
    }
    let age = ms(SystemTime::now()).saturating_sub(at);
    let may_wait = ctx.running && age < WAIT.as_millis() as u64;
    let user = report.get("replay").and_then(Value::as_str).map(|r| user_of(r).to_string()).unwrap_or_default();
    let Some(rec) = ctx.demos.as_deref().and_then(|d| pick(&recordings(d), at, &user)) else {
        return note(report, "missing");
    };
    if !rec.finished && may_wait {
        return Step::Wait;
    }
    let link = |report: &mut Map<String, Value>, url: &str, bytes: u64| {
        report.insert("replay_url".into(), url.into());
        report.insert("replay_bytes".into(), bytes.into());
        report.insert("replay_match".into(), header_name(&rec.dir).into());
        Step::Done
    };
    if let Some(before) = sent(reports).get(&sent_key(&rec)) {
        if let Some(url) = before.get("url").and_then(Value::as_str) {
            return link(report, url, before.get("bytes").and_then(Value::as_u64).unwrap_or(0));
        }
    }
    let dir = rec.dir.clone();
    let zipped = tokio::task::spawn_blocking(move || zip(&dir)).await.unwrap_or(Err(ZipProblem::Unreadable));
    let body = match zipped {
        Ok(body) => body,
        Err(ZipProblem::TooBig) => return note(report, "too_big"),
        // The game may still hold a file of it: once more on the next pass.
        Err(ZipProblem::Unreadable) if may_wait => return Step::Wait,
        Err(ZipProblem::Unreadable) => return note(report, "failed"),
    };
    match upload(ctx, session, &rec.dir, body).await {
        Upload::Stored { url, bytes } => {
            remember(reports, &rec, &url, bytes);
            link(report, &url, bytes)
        }
        Upload::TooBig => note(report, "too_big"),
        Upload::Refused => note(report, "failed"),
        Upload::Later if may_wait => Step::Wait,
        Upload::Later => note(report, "failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recording as the game leaves it: `MK3D.demo`, `.header`, a checkpoint,
    /// `.replayinfo` (unless `info` is None), `.final` when over.
    fn recording(demos: &Path, name: &str, info: Option<(u64, u64, bool, &str)>, over: bool) -> PathBuf {
        let dir = demos.join(name);
        std::fs::create_dir_all(dir.join("checkpoints")).unwrap();
        std::fs::write(dir.join("MK3D.demo"), vec![7u8; 50_000]).unwrap();
        std::fs::write(dir.join("MK3D.header"), b"header").unwrap();
        std::fs::write(dir.join("checkpoints").join("checkpoint0"), b"checkpoint").unwrap();
        if let Some((start, length, live, user)) = info {
            let json = format!(
                "{{\r\n\t\"LengthInMS\": {length},\r\n\t\"Timestamp\": {start},\r\n\t\"bIsLive\": {live},\r\n\t\"RecordUserId\": \"{user}\",\r\n\t\"MapName\": \"LV-OrbIsland\"\r\n}}"
            );
            let mut bytes = ((json.len() + 1) as i32).to_le_bytes().to_vec();
            bytes.extend_from_slice(json.as_bytes());
            bytes.push(0);
            std::fs::write(dir.join("MK3D.replayinfo"), bytes).unwrap();
        }
        if over {
            std::fs::write(dir.join("MK3D.final"), b"").unwrap();
        }
        dir
    }

    const ME: &str = "f5a441e76deab1b92db6074e0d6ab07c";
    const MIN: u64 = 60_000;

    #[test]
    fn the_replay_info_is_read_as_the_game_writes_it() {
        let demos = tempfile::tempdir().unwrap();
        let dir = recording(demos.path(), "kapi_2026-10-01_20-25", Some((1_790_900_703_530, 911_834, false, ME)), true);
        let rec = read(&dir).unwrap();
        assert_eq!(rec.start, Some(1_790_900_703_530));
        assert_eq!(rec.length, 911_834);
        assert_eq!(rec.user, ME);
        assert!(rec.finished);
        assert_eq!(user_of(&format!("{ME}_r__103_2026-10-02_00_37_03.7z")), ME);
        assert_eq!(user_of("whatever.7z"), "");
        // A folder without a recording in it is none.
        std::fs::create_dir_all(demos.path().join("empty")).unwrap();
        assert!(read(&demos.path().join("empty")).is_none());
    }

    #[test]
    fn a_report_belongs_to_the_match_that_ran_when_it_was_made() {
        let demos = tempfile::tempdir().unwrap();
        let t0 = 1_790_900_000_000;
        recording(demos.path(), "kapi_a", Some((t0, 20 * MIN, false, ME)), true);
        recording(demos.path(), "kapi_b", Some((t0 + 30 * MIN, 15 * MIN, false, ME)), true);
        recording(demos.path(), "other_c", Some((t0 + 30 * MIN, 15 * MIN, false, "0123456789abcdef0123456789abcdef")), true);
        let list = recordings(demos.path());
        let name = |r: Option<Recording>| r.map(|r| r.dir.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(name(pick(&list, t0 + 5 * MIN, ME)), Some("kapi_a".into()));
        assert_eq!(name(pick(&list, t0 + 37 * MIN, ME)), Some("kapi_b".into()), "the second match, not the other account's");
        assert_eq!(name(pick(&list, t0 + 37 * MIN, "0123456789abcdef0123456789abcdef")), Some("other_c".into()));
        // Between matches (the lobby, or watching an old replay): none of them.
        assert_eq!(pick(&list, t0 + 25 * MIN, ME), None);
        assert_eq!(pick(&list, t0 + 90 * MIN, ME), None);
    }

    #[test]
    fn a_match_still_being_played_is_found_but_not_finished() {
        let demos = tempfile::tempdir().unwrap();
        let now = ms(SystemTime::now());
        recording(demos.path(), "kapi_now", Some((now - 5 * MIN, 0, true, ME)), false);
        let found = pick(&recordings(demos.path()), now - MIN, ME).expect("the match in progress");
        assert!(!found.finished);
        // Without its replay info yet, what was just written still counts.
        let bare = tempfile::tempdir().unwrap();
        recording(bare.path(), "kapi_bare", None, false);
        assert!(pick(&recordings(bare.path()), now - MIN, ME).is_some());
        // An old recording a crash left unfinished is not today's match.
        assert!(pick(&recordings(bare.path()), now + 60 * MIN, ME).is_none());
    }

    #[test]
    fn the_zip_holds_the_recordings_folder_as_it_is() {
        let demos = tempfile::tempdir().unwrap();
        let dir = recording(demos.path(), "kapi_2026-10-01_20-25", Some((1, 2, false, ME)), true);
        let bytes = zip(&dir).unwrap();
        assert!(bytes.len() < 50_000, "compressed: {}", bytes.len());
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut names: Vec<String> = (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "kapi_2026-10-01_20-25/MK3D.demo",
                "kapi_2026-10-01_20-25/MK3D.final",
                "kapi_2026-10-01_20-25/MK3D.header",
                "kapi_2026-10-01_20-25/MK3D.replayinfo",
                "kapi_2026-10-01_20-25/checkpoints/checkpoint0",
            ]
        );
        let mut demo = Vec::new();
        std::io::Read::read_to_end(&mut archive.by_name("kapi_2026-10-01_20-25/MK3D.demo").unwrap(), &mut demo).unwrap();
        assert_eq!(demo, vec![7u8; 50_000]);
    }

    #[test]
    fn a_header_never_carries_more_than_a_plain_name() {
        assert_eq!(header_name(Path::new("C:/x/kapi_2026-10-01_20-25")), "kapi_2026-10-01_20-25");
        assert_eq!(header_name(Path::new("C:/x/Jörg é\r\nX")), "J_rg____X");
    }
}
