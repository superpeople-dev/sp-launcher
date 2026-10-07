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
//! menu) and uploaded to the website (sp-website lib/replays.ts): it hands
//! out an upload link, and the zip goes straight into its storage. The report
//! then carries the replay's page, https://superpeople.dev/replays/<id>, which
//! needs no sign-in and is kept 30 days.
//!
//! A report made during a match waits for that match to end, but not longer
//! than [`WAIT`]: the game (its MK3D replay streamer) keeps the recording in
//! memory and writes the whole folder at once when the match is over, so until
//! then there is nothing in Demos to find. 07.10.2026: reports made in a match
//! went out at once with "no recording", since nothing was there yet.
//!
//! A report made in the game's Replay menu is about the recording being
//! watched, an earlier match. The game logs the one picked there
//! (`LogTemp: Display: Selected ReplayName = <its folder>`), so the last such
//! line before the report names it ([`watched`]).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// What the website takes at most (sp-website lib/replays.ts MAX_BYTES). A
/// whole match zips to 2-15 MB.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// More than this unzipped is no recording of one match: not even zipped.
const MAX_RAW: u64 = 1024 * 1024 * 1024;
/// How far apart the report's clock and the recording's may be.
const SLACK_MS: u64 = 90_000;
/// How long a report waits for its match to end, or for the website to take
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

/// The game's log folder.
pub fn logs_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(Path::new(&base).join("BravoHotelGame").join("Saved").join("Logs"))
}

/// Where a replay comes from and goes to.
pub struct Ctx {
    /// The game's Demos folder; `None` when there is none to look in.
    pub demos: Option<PathBuf>,
    /// The game's log folder, which says what the Replay menu played.
    pub logs: Option<PathBuf>,
    /// The website, which hands out the upload link (`/api/launcher/replays`).
    pub site: String,
    /// Whether the game is still running: a report may then wait for its match.
    pub running: bool,
}

impl Ctx {
    pub fn live(running: bool) -> Self {
        Ctx { demos: demos_dir(), logs: logs_dir(), site: crate::auth::site_url(), running }
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

/// The game's EReportType for a report made in its Replay menu.
const FROM_REPLAY_MENU: u64 = 2;

/// What the game logs when a replay is picked in its Replay menu (the replay
/// list's widget, RVA 0x182AA20, LogTemp at Display): the recording's folder.
const SELECTED: &str = "LogTemp: Display: Selected ReplayName = ";

/// `[2026.10.07-22.15.42:745]` at the start of a game log line, Unreal's UTC,
/// as ms since 1970.
fn ue_stamp(line: &str) -> Option<u64> {
    let s = line.as_bytes();
    if s.len() < 25 || s[0] != b'[' || s[24] != b']' {
        return None;
    }
    let num = |a: usize, b: usize| line.get(a..b).and_then(|t| t.parse::<i64>().ok());
    let (y, mo, d) = (num(1, 5)?, num(6, 8)?, num(9, 11)?);
    let (h, mi, sec, milli) = (num(12, 14)?, num(15, 17)?, num(18, 20)?, num(21, 24)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    // Days since 1970 of a proleptic Gregorian date (Howard Hinnant's days_from_civil).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + milli).ok()
}

/// The recording last picked in the game's Replay menu before `at` (ms): its
/// folder's name, from the `Selected ReplayName` lines of the game's logs
/// written since then (the current one and its backups, so a report sent after
/// the game was closed or started again still finds it).
pub fn watched(logs: &Path, at: u64) -> Option<String> {
    use std::io::BufRead;
    let Ok(entries) = std::fs::read_dir(logs) else { return None };
    let files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("BravoHotelGame") && name.ends_with(".log") && !name.contains("Critical")
        })
        .filter(|e| e.metadata().and_then(|m| m.modified()).map(ms).is_ok_and(|t| t + SLACK_MS >= at))
        .map(|e| e.path())
        .collect();
    let mut best: Option<(u64, String)> = None;
    for file in files {
        let Ok(f) = std::fs::File::open(&file) else { continue };
        let mut reader = std::io::BufReader::new(f);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            let Some(i) = line.find(SELECTED) else { continue };
            let name = line[i + SELECTED.len()..].trim();
            let Some(stamp) = ue_stamp(&line) else { continue };
            if stamp <= at + SLACK_MS && folder_ok(name) && best.as_ref().is_none_or(|(t, _)| stamp >= *t) {
                best = Some((stamp, name.to_string()));
            }
        }
    }
    best.map(|(_, name)| name)
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
    /// The website read it and will not take it, or has no place for it.
    Refused,
    /// Offline, signed out, the website busy: the same replay may go later.
    Later,
}

/// The website's answer: where to put the zip (a short-lived link into its
/// storage) and the replay's page.
#[derive(serde::Deserialize)]
struct Started {
    upload_url: String,
    url: String,
}

/// What the recording is called in a header: its folder's name, plain.
fn header_name(dir: &Path) -> String {
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).take(80).collect()
}

/// The zip onto the website, with the player's own session (as for reports):
/// it hands out an upload link and the replay's page, and the zip goes straight
/// into its storage. The website checks the file when the report arrives.
async fn upload(ctx: &Ctx, session: &str, dir: &Path, body: Vec<u8>) -> Upload {
    let Ok(client) = crate::auth::client() else { return Upload::Later };
    let bytes = body.len() as u64;
    let res = client
        .post(format!("{}/api/launcher/replays", ctx.site))
        .bearer_auth(session)
        .json(&serde_json::json!({ "name": header_name(dir), "bytes": bytes }))
        .send()
        .await;
    let Ok(res) = res else { return Upload::Later };
    let started = match res.status().as_u16() {
        200..=299 => match res.json::<Started>().await {
            Ok(s) if s.url.starts_with("https://") && s.upload_url.starts_with("http") => s,
            _ => return Upload::Refused,
        },
        413 => return Upload::TooBig,
        // Signed out, too many this hour, the website or its storage busy.
        401 | 429 | 500..=599 => return Upload::Later,
        _ => return Upload::Refused,
    };
    let put = client
        .put(&started.upload_url)
        .timeout(UPLOAD_TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/zip")
        .body(body)
        .send()
        .await;
    match put {
        Ok(res) if res.status().is_success() => Upload::Stored { url: started.url, bytes },
        // A link that ran out or a storage hiccup: the next pass asks for a new one.
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
    let list = ctx.demos.as_deref().map(recordings).unwrap_or_default();
    // Made in the Replay menu: the recording being watched, whichever match it was.
    let from_menu = report.get("type").and_then(Value::as_u64) == Some(FROM_REPLAY_MENU);
    let in_menu = || {
        let name = ctx.logs.as_deref().and_then(|l| watched(l, at))?;
        list.iter().find(|r| r.dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(&name))).cloned()
    };
    let found = if from_menu { in_menu() } else { None }.or_else(|| pick(&list, at, &user));
    let Some(rec) = found else {
        // A match's recording appears only once the match is over (the game writes it all
        // then): while the game runs, wait for it. Not for one from the Replay menu, which is
        // there already or never comes, nor once a match that began after the report has been
        // written, since the reported match ended before it and left nothing.
        let later_match = list.iter().any(|r| r.start.is_some_and(|s| s > at + SLACK_MS));
        if may_wait && !from_menu && !later_match {
            return Step::Wait;
        }
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

// --------------------------------------------- an admin opening a replay ---
// The staff's link to a reported match opens its page on the website
// (superpeople.dev/replays/<id>, no sign-in: the id is the secret), whose "Open
// in the launcher" is sp-launcher://replay/<id>. Windows hands that to the
// launcher (lib.rs, the deep-link plugin); after the admin says yes, the replay
// is downloaded from the website and unzipped into the game's Demos folder,
// where the game's Replay menu lists it. Replays from before (the admin panel's
// page, behind its sign-in) come as sp-launcher://replay/<id>?t=<token> and are
// downloaded from the game backend with that token (it works once, for 10
// minutes).

/// A replay link: the replay's id, and the one-time token of an admin-panel
/// link ("" for one from the website).
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub id: String,
    pub token: String,
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `sp-launcher://replay/<32 hex>`, or with `?t=<hex>` (an admin-panel link),
/// or None for anything else.
pub fn link_of(url: &str) -> Option<Link> {
    let url = tauri::Url::parse(url).ok()?;
    if url.scheme() != "sp-launcher" || url.host_str()? != "replay" {
        return None;
    }
    let id = url.path().trim_matches('/');
    let token = match url.query_pairs().find(|(k, _)| k == "t") {
        Some((_, t)) => {
            let t = t.into_owned();
            if !(16..=128).contains(&t.len()) || !is_hex(&t) {
                return None;
            }
            t
        }
        None => String::new(),
    };
    (id.len() == 32 && is_hex(id)).then(|| Link { id: id.to_ascii_lowercase(), token })
}

#[derive(Debug, PartialEq)]
pub enum Fetched {
    Zip(Vec<u8>),
    /// An admin-panel link used already, or older than 10 minutes.
    Expired,
    /// No longer kept (replays are kept 30 days).
    Gone,
    TooBig,
    Failed,
}

/// The replay's zip: from the website (its download sends on to its storage),
/// or for an admin-panel link from the game backend with the link's token.
pub async fn fetch(site: &str, backend: &str, link: &Link) -> Fetched {
    let Ok(client) = crate::auth::client() else { return Fetched::Failed };
    let req = if link.token.is_empty() {
        client.get(format!("{site}/replays/{}/download", link.id))
    } else {
        client.get(format!("{backend}/replays/{}/file", link.id)).query(&[("t", link.token.as_str())])
    };
    let res = req.timeout(UPLOAD_TIMEOUT).send().await;
    let Ok(mut res) = res else { return Fetched::Failed };
    match res.status().as_u16() {
        200 => {}
        403 => return Fetched::Expired,
        404 => return Fetched::Gone,
        _ => return Fetched::Failed,
    }
    if res.content_length().is_some_and(|n| n > MAX_BYTES) {
        return Fetched::TooBig;
    }
    let mut body = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() as u64 > MAX_BYTES {
                    return Fetched::TooBig;
                }
            }
            Ok(None) => return Fetched::Zip(body),
            Err(_) => return Fetched::Failed,
        }
    }
}

/// Where an opened replay went: its folder's name in Demos, and whether it was
/// there already.
#[derive(Debug, PartialEq, serde::Serialize)]
pub struct Imported {
    pub name: String,
    pub already: bool,
}

#[derive(Debug, PartialEq)]
pub enum ImportProblem {
    /// Not one recording's folder (one folder at the top, with its .replayinfo).
    NotReplay,
    TooBig,
    /// The Demos folder could not be written.
    Write,
}

/// A folder name the game's own recordings have, and Windows takes.
fn folder_ok(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 120
        && !name.starts_with('.')
        && !name.ends_with(['.', ' '])
        && !name.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
}

/// Unzips a replay (one recording's folder, as `zip` makes it) into `demos`,
/// nothing outside its folder. Written to a hidden `.<name>.part` first and
/// renamed when whole, so the game never lists half of one. A recording that is
/// there already is left as it is.
pub fn import(bytes: &[u8], demos: &Path) -> Result<Imported, ImportProblem> {
    use std::io::Read;
    use std::path::Component;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| ImportProblem::NotReplay)?;
    if archive.len() == 0 || archive.len() > 10_000 {
        return Err(ImportProblem::NotReplay);
    }
    // One folder at the top, with the recording's .replayinfo right in it.
    let mut top: Option<String> = None;
    let mut has_info = false;
    let mut raw: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|_| ImportProblem::NotReplay)?;
        // As written in the zip: a drive (`a:b`) or a backslash is no recording's path,
        // even where the zip library would make one of it.
        if entry.name().contains([':', '\\']) {
            return Err(ImportProblem::NotReplay);
        }
        let path = entry.enclosed_name().ok_or(ImportProblem::NotReplay)?;
        let parts: Vec<Component> = path.components().collect();
        let first = match parts.first() {
            Some(Component::Normal(n)) => n.to_string_lossy().into_owned(),
            _ => return Err(ImportProblem::NotReplay),
        };
        if !folder_ok(&first) || parts.iter().skip(1).any(|c| !matches!(c, Component::Normal(_))) {
            return Err(ImportProblem::NotReplay);
        }
        match &top {
            None => top = Some(first),
            Some(t) if *t == first => {}
            Some(_) => return Err(ImportProblem::NotReplay),
        }
        if parts.len() == 2 && !entry.is_dir() && path.extension().is_some_and(|e| e == "replayinfo") {
            has_info = true;
        }
        raw += entry.size();
        if raw > MAX_RAW {
            return Err(ImportProblem::TooBig);
        }
    }
    let name = top.ok_or(ImportProblem::NotReplay)?;
    if !has_info {
        return Err(ImportProblem::NotReplay);
    }
    let dest = demos.join(&name);
    if dest.exists() {
        return Ok(Imported { name, already: true });
    }
    std::fs::create_dir_all(demos).map_err(|_| ImportProblem::Write)?;
    let part = demos.join(format!(".{name}.part"));
    let _ = std::fs::remove_dir_all(&part);
    let mut write = || -> Result<(), ImportProblem> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|_| ImportProblem::NotReplay)?;
            let path = entry.enclosed_name().ok_or(ImportProblem::NotReplay)?;
            let rel = path.strip_prefix(&name).map_err(|_| ImportProblem::NotReplay)?.to_path_buf();
            let out = part.join(&rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|_| ImportProblem::Write)?;
                continue;
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|_| ImportProblem::Write)?;
            }
            let size = entry.size();
            let mut file = std::fs::File::create(&out).map_err(|_| ImportProblem::Write)?;
            // No more than the entry says it holds, whatever its compressed data unpacks to.
            let copied = std::io::copy(&mut (&mut entry).take(size + 1), &mut file).map_err(|_| ImportProblem::NotReplay)?;
            if copied > size {
                return Err(ImportProblem::NotReplay);
            }
        }
        Ok(())
    };
    if let Err(problem) = write() {
        let _ = std::fs::remove_dir_all(&part);
        return Err(problem);
    }
    if std::fs::rename(&part, &dest).is_err() {
        let _ = std::fs::remove_dir_all(&part);
        return Err(ImportProblem::Write);
    }
    Ok(Imported { name, already: false })
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
    fn a_log_lines_time_is_read_as_unreal_writes_it() {
        // The game's own: this line came with the recording whose Timestamp is 1791410441152.
        assert_eq!(ue_stamp("[2026.10.07-22.00.41:152][873]LogDemo: Warning: x"), Some(1_791_410_441_152));
        assert_eq!(ue_stamp("[1970.01.01-00.00.00:000][  0]x"), Some(0));
        assert_eq!(ue_stamp("[2024.02.29-12.00.00:000][  0]x"), Some(1_709_208_000_000));
        assert_eq!(ue_stamp("LogTemp: Display: no time"), None);
        assert_eq!(ue_stamp("[2026.13.07-22.00.41:152][  0]x"), None);
    }

    /// A game log with these lines, last written at `modified` (ms).
    fn game_log(dir: &Path, name: &str, lines: &[&str], modified: u64) {
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\r\n") + "\r\n").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_millis(modified)).unwrap();
    }

    /// 2026.10.07-22.00.41:152 UTC.
    const AT: u64 = 1_791_410_441_152;

    #[test]
    fn the_replay_menu_names_the_recording_being_watched() {
        let logs = tempfile::tempdir().unwrap();
        game_log(
            logs.path(),
            "BravoHotelGame.log",
            &[
                "[2026.10.07-21.50.00:000][  1]LogTemp: Display: Selected ReplayName = kapi_2026-10-01_20-25",
                "[2026.10.07-21.55.00:000][  2]LogTemp: Display: Selected ReplayName = kapi_2026-10-03_21-54",
                "[2026.10.07-21.56.00:000][  3]LogTemp: Display: Selected ReplayName = ..\\..\\Windows",
                "[2026.10.07-21.57.00:000][  4]LogDemo: Display: anything else",
                "[2026.10.07-22.10.00:000][  5]LogTemp: Display: Selected ReplayName = kapi_picked_later",
            ],
            AT + 20 * MIN,
        );
        // A game before this one: its log was done before the report was made.
        game_log(
            logs.path(),
            "BravoHotelGame-backup-2026.10.06-10.00.00.log",
            &["[2026.10.07-21.58.00:000][  1]LogTemp: Display: Selected ReplayName = kapi_old_game"],
            AT - 24 * 60 * MIN,
        );
        assert_eq!(watched(logs.path(), AT).as_deref(), Some("kapi_2026-10-03_21-54"), "the last one picked before the report, a real folder name");
        assert_eq!(watched(logs.path(), AT - 30 * MIN), None, "nothing picked yet then");
        assert_eq!(watched(&logs.path().join("none"), AT), None);
    }

    fn test_ctx(demos: &Path, logs: Option<&Path>, running: bool) -> Ctx {
        Ctx { demos: Some(demos.to_path_buf()), logs: logs.map(Path::to_path_buf), site: "http://127.0.0.1:9".into(), running }
    }

    /// A replay sent before, so linking it asks the website nothing.
    fn sent_before(reports: &Path, name: &str, start: u64) {
        let list = serde_json::json!({ format!("{name}|{start}"): { "url": "https://superpeople.dev/replays/abab", "bytes": 7, "at": ms(SystemTime::now()) } });
        std::fs::write(reports.join(SENT_FILE), list.to_string()).unwrap();
    }

    #[tokio::test]
    async fn a_report_made_in_a_match_waits_for_its_recording_while_the_game_runs() {
        let (demos, reports) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let now = ms(SystemTime::now());
        let at = now - MIN;
        // The match is still on: the game has written nothing of it yet.
        let mut report = Map::new();
        assert_eq!(attach(&test_ctx(demos.path(), None, true), "s", reports.path(), &mut report, at).await, Step::Wait);
        assert!(report.is_empty());
        // The game was closed and never wrote it: the report goes without.
        assert_eq!(attach(&test_ctx(demos.path(), None, false), "s", reports.path(), &mut report, at).await, Step::Done);
        assert_eq!(report.get("replay_note").and_then(Value::as_str), Some("missing"));
        // A match that began after the report has been written: the reported one left nothing.
        let next = recording(demos.path(), "kapi_next", Some((now + 5 * MIN, 10 * MIN, false, ME)), true);
        let mut report = Map::new();
        assert_eq!(attach(&test_ctx(demos.path(), None, true), "s", reports.path(), &mut report, at).await, Step::Done);
        assert_eq!(report.get("replay_note").and_then(Value::as_str), Some("missing"));
        std::fs::remove_dir_all(next).unwrap();
        // The match is over and its recording is there: the report links it.
        recording(demos.path(), "kapi_this", Some((at - 5 * MIN, 20 * MIN, false, ME)), true);
        sent_before(reports.path(), "kapi_this", at - 5 * MIN);
        let mut report = Map::new();
        assert_eq!(attach(&test_ctx(demos.path(), None, true), "s", reports.path(), &mut report, at).await, Step::Done);
        assert_eq!(report.get("replay_match").and_then(Value::as_str), Some("kapi_this"));
        assert_eq!(report.get("replay_url").and_then(Value::as_str), Some("https://superpeople.dev/replays/abab"));
    }

    #[tokio::test]
    async fn a_report_from_the_replay_menu_takes_the_recording_being_watched() {
        let (demos, reports, logs) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // A match of three days before, watched in the Replay menu.
        let start = AT - 3 * 24 * 60 * MIN;
        recording(demos.path(), "kapi_2026-10-04_20-05", Some((start, 20 * MIN, false, ME)), true);
        sent_before(reports.path(), "kapi_2026-10-04_20-05", start);
        game_log(
            logs.path(),
            "BravoHotelGame.log",
            &["[2026.10.07-21.55.00:000][  2]LogTemp: Display: Selected ReplayName = kapi_2026-10-04_20-05"],
            AT + MIN,
        );
        let mut report = Map::new();
        report.insert("type".into(), FROM_REPLAY_MENU.into());
        assert_eq!(attach(&test_ctx(demos.path(), Some(logs.path()), false), "s", reports.path(), &mut report, AT).await, Step::Done);
        assert_eq!(report.get("replay_match").and_then(Value::as_str), Some("kapi_2026-10-04_20-05"));
        // The same report made in a match (death cam): no match ran then, and the menu does not count.
        let mut report = Map::new();
        report.insert("type".into(), 3.into());
        assert_eq!(attach(&test_ctx(demos.path(), Some(logs.path()), false), "s", reports.path(), &mut report, AT).await, Step::Done);
        assert_eq!(report.get("replay_note").and_then(Value::as_str), Some("missing"));
        // From the Replay menu with nothing in the log: no waiting, even while the game runs.
        let now = ms(SystemTime::now());
        let mut report = Map::new();
        report.insert("type".into(), FROM_REPLAY_MENU.into());
        assert_eq!(attach(&test_ctx(demos.path(), None, true), "s", reports.path(), &mut report, now - MIN).await, Step::Done);
        assert_eq!(report.get("replay_note").and_then(Value::as_str), Some("missing"));
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

    // ------------------------------------------- an admin opening a replay ---

    const ID: &str = "0123456789abcdef0123456789abcdef";
    const TOKEN: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718";

    #[test]
    fn a_replay_link_is_read_and_anything_else_is_not() {
        let want = Some(Link { id: ID.into(), token: TOKEN.into() });
        assert_eq!(link_of(&format!("sp-launcher://replay/{ID}?t={TOKEN}")), want);
        // Windows or a browser may add a slash before the query; the id in capitals is the same id.
        assert_eq!(link_of(&format!("sp-launcher://replay/{ID}/?t={TOKEN}")), want);
        assert_eq!(link_of(&format!("sp-launcher://replay/{}?t={TOKEN}", ID.to_uppercase())), want);
        // The website's links have no token: the id is the secret.
        let site = Some(Link { id: ID.into(), token: String::new() });
        assert_eq!(link_of(&format!("sp-launcher://replay/{ID}")), site);
        assert_eq!(link_of(&format!("sp-launcher://replay/{ID}/")), site);
        for bad in [
            format!("https://replay/{ID}?t={TOKEN}"),
            format!("sp-launcher://other/{ID}?t={TOKEN}"),
            format!("sp-launcher://other/{ID}"),
            format!("sp-launcher://replay/{ID}?t=short"),
            format!("sp-launcher://replay/{ID}?t=not-hex-not-hex-not-hex"),
            format!("sp-launcher://replay/..%2F..%2Fx?t={TOKEN}"),
            format!("sp-launcher://replay/{ID}x?t={TOKEN}"),
            "not a url".to_string(),
        ] {
            assert_eq!(link_of(&bad), None, "{bad}");
        }
    }

    /// Answers each request with the next (status, extra header line, body), and
    /// hands back the request lines.
    async fn answers(replies: Vec<(u16, String, Vec<u8>)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let task = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, header, body) in replies {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string());
                let mut reply = format!("HTTP/1.1 {status} X\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                reply.extend_from_slice(&body);
                let _ = sock.write_all(&reply).await;
            }
            seen
        });
        (base, task)
    }

    #[tokio::test]
    async fn a_website_link_downloads_through_its_redirect_and_an_old_one_with_its_token() {
        let zip = vec![b'P', b'K', 3, 4, 1, 2, 3];
        // The website's download sends on to its storage, which has the zip.
        let (base, server) = answers(vec![]).await;
        drop(server);
        let (storage, storage_seen) = answers(vec![(200, "Content-Type: application/zip\r\n".into(), zip.clone())]).await;
        let (site, site_seen) = answers(vec![(302, format!("Location: {storage}/media/replays/{ID}/kapi.zip?sig=1\r\n"), vec![])]).await;
        let link = Link { id: ID.into(), token: String::new() };
        assert_eq!(fetch(&site, &base, &link).await, Fetched::Zip(zip.clone()));
        assert!(site_seen.await.unwrap()[0].starts_with(&format!("GET /replays/{ID}/download ")));
        assert!(storage_seen.await.unwrap()[0].starts_with(&format!("GET /media/replays/{ID}/kapi.zip?sig=1 ")));

        // Gone after 30 days.
        let (site, _) = answers(vec![(404, String::new(), b"gone".to_vec())]).await;
        assert_eq!(fetch(&site, &base, &link).await, Fetched::Gone);

        // An admin-panel link goes to the game backend with its token.
        let (backend, backend_seen) = answers(vec![(200, String::new(), zip.clone())]).await;
        let old = Link { id: ID.into(), token: TOKEN.into() };
        assert_eq!(fetch("http://127.0.0.1:9", &backend, &old).await, Fetched::Zip(zip));
        assert!(backend_seen.await.unwrap()[0].starts_with(&format!("GET /replays/{ID}/file?t={TOKEN} ")));
    }

    /// A zip of these (name, bytes) entries, as written by anyone.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            out.start_file(*name, options).unwrap();
            out.write_all(bytes).unwrap();
        }
        out.finish().unwrap().into_inner()
    }

    #[test]
    fn a_replay_opened_from_its_link_lands_in_demos_as_it_was_recorded() {
        let mine = tempfile::tempdir().unwrap();
        let dir = recording(mine.path(), "Inquisitor1961_2026-10-02_20-48", Some((1, 2, false, ME)), true);
        let bytes = zip(&dir).unwrap();
        let theirs = tempfile::tempdir().unwrap();
        let demos = theirs.path().join("Saved").join("Demos");
        let got = import(&bytes, &demos).unwrap();
        assert_eq!(got, Imported { name: "Inquisitor1961_2026-10-02_20-48".into(), already: false });
        let out = demos.join("Inquisitor1961_2026-10-02_20-48");
        assert_eq!(std::fs::read(out.join("MK3D.demo")).unwrap(), vec![7u8; 50_000]);
        assert_eq!(std::fs::read(out.join("checkpoints").join("checkpoint0")).unwrap(), b"checkpoint");
        assert!(out.join("MK3D.replayinfo").exists() && out.join("MK3D.final").exists());
        // Nothing half-written left beside it.
        let names: Vec<_> = std::fs::read_dir(&demos).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "{names:?}");

        // Again: there already, and left as it is.
        std::fs::write(out.join("MK3D.header"), b"mine").unwrap();
        assert_eq!(import(&bytes, &demos).unwrap(), Imported { name: "Inquisitor1961_2026-10-02_20-48".into(), already: true });
        assert_eq!(std::fs::read(out.join("MK3D.header")).unwrap(), b"mine");
    }

    #[test]
    fn a_zip_that_is_not_one_recording_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let demos = tmp.path().join("Demos");
        let info: &[u8] = b"{}";
        for (why, bytes) in [
            ("no zip at all", b"not a zip".to_vec()),
            ("no .replayinfo", zip_of(&[("a_2026/MK3D.demo", b"x")])),
            ("two folders", zip_of(&[("a_2026/MK3D.replayinfo", info), ("b_2026/MK3D.demo", b"x")])),
            ("a file at the top", zip_of(&[("MK3D.replayinfo", info)])),
            ("out of its folder", zip_of(&[("a_2026/MK3D.replayinfo", info), ("a_2026/../../evil.txt", b"x")])),
            ("an absolute path", zip_of(&[("a_2026/MK3D.replayinfo", info), ("/evil.txt", b"x")])),
            ("a hidden folder", zip_of(&[(".a/MK3D.replayinfo", info)])),
            ("a name Windows refuses", zip_of(&[("a:b/MK3D.replayinfo", info)])),
            ("the .replayinfo deeper down", zip_of(&[("a_2026/sub/MK3D.replayinfo", info)])),
        ] {
            assert_eq!(import(&bytes, &demos), Err(ImportProblem::NotReplay), "{why}");
        }
        assert!(!demos.exists() || std::fs::read_dir(&demos).unwrap().next().is_none(), "something was written");
        assert!(!tmp.path().join("evil.txt").exists());
    }
}
