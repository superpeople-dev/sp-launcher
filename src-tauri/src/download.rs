//! The Download tab: fetches the game from archive.org, verifies it, unpacks it
//! and points the launcher at the result.
//!
//! WHAT IT DOWNLOADS
//! -----------------
//! One file: `Manifest #2065353802481281242.7z` from the archive.org item
//! `SPShippingDev` ("SUPER PEOPLE Testing Grounds [S-DEV]"), ~27.7 GB. Size and
//! MD5 are NOT hardcoded -- they are read from archive.org's metadata API at the
//! start of every run, so a re-upload of the item cannot leave the launcher
//! checking against a stale hash.
//!
//! THE PIPELINE
//! ------------
//!   checking    metadata + free-space check
//!   downloading HTTP GET with `Range`, appended to `<dir>\.sp-download\<file>.part`
//!   verifying   MD5 of the finished file against archive.org's value
//!   extracting  7-Zip (`7za.exe` embedded at build time, or an installed 7-Zip)
//!   done        install folder set, archive and temp folder deleted
//!
//! PAUSE / RESUME
//! --------------
//! Pause stops the task and keeps the `.part` file. Continue sends a `Range`
//! request from the current length. The same happens after a launcher restart
//! or a crash: `download.v1.json` in the config dir remembers the target folder,
//! and the length of the `.part` file IS the progress -- there is no separate
//! counter that could disagree with the disk. A server that answers a range
//! request with a full `200` is detected and the file is restarted from zero
//! rather than corrupted by appending a second copy.
//!
//! Network errors are retried automatically (backoff 2 s .. 30 s, 25 tries per
//! run) because archive.org regularly drops connections on multi-GB transfers;
//! a stalled stream (no byte for 45 s) counts as an error.
//!
//! While a download or extraction runs, Windows is asked not to go to sleep
//! (`SetThreadExecutionState`); the display may still turn off.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{LauncherError, Result};

// ----------------------------------------------------------------- source ---

pub const ITEM_ID: &str = "SPShippingDev";
pub const FILE_NAME: &str = "Manifest #2065353802481281242.7z";
/// `FILE_NAME` percent-encoded (space and '#').
const FILE_URL: &str = "https://archive.org/download/SPShippingDev/Manifest%20%232065353802481281242.7z";
const META_URL: &str = "https://archive.org/metadata/SPShippingDev/files";

/// Before the archive is downloaded its unpacked size is unknown; this is the
/// estimate used for the first free-space check. The game is mostly .pak
/// files that are already compressed, so 7z gains little on them -- 1.3x is
/// deliberately on the safe side. The exact number is read from the archive
/// itself before extraction and checked again.
const UNPACKED_ESTIMATE: f64 = 1.3;
/// Headroom on top of every free-space requirement.
const SPACE_MARGIN: u64 = 2 * 1024 * 1024 * 1024;

const STATE_FILE: &str = "download.v1.json";
const TEMP_DIR: &str = ".sp-download";
const MAX_RETRIES: u32 = 25;
const STALL_TIMEOUT: Duration = Duration::from_secs(45);
const EMIT_EVERY: Duration = Duration::from_millis(250);

// ------------------------------------------------------------------ types ---

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nothing started, or a previous run was cancelled.
    Idle,
    Checking,
    Downloading,
    /// Stopped by the user (or by a launcher restart); the `.part` file stays.
    Paused,
    Verifying,
    Extracting,
    Done,
    Failed,
}

/// What the Download tab draws. Sent as the `download:status` event while a
/// run is active, and returned by `download_status` at any time.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub phase: Phase,
    /// Target folder the player chose. Empty until one is picked.
    pub dir: String,
    /// Bytes done in the CURRENT step (downloaded / hashed / unpacked).
    pub done: u64,
    /// Size of the current step. 0 while unknown.
    pub total: u64,
    /// Bytes per second, smoothed. 0 when not transferring.
    pub speed: f64,
    /// Seconds left in the current step, if it can be estimated.
    pub eta_secs: Option<u64>,
    /// Human-readable line under the bar (errors, "Retrying in 8 s", ...).
    pub message: String,
    /// Free space on the target drive, and what the whole install needs.
    pub free_bytes: Option<u64>,
    pub needed_bytes: Option<u64>,
    pub retries: u32,
    /// Where the finished game landed (Done only).
    pub install_dir: String,
}

impl Status {
    fn idle(dir: String) -> Self {
        Status {
            phase: Phase::Idle,
            dir,
            done: 0,
            total: 0,
            speed: 0.0,
            eta_secs: None,
            message: String::new(),
            free_bytes: None,
            needed_bytes: None,
            retries: 0,
            install_dir: String::new(),
        }
    }
}

/// Survives restarts. The `.part` file length is the progress; this only
/// remembers WHERE, and what the file is supposed to be.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    dir: String,
    total: u64,
    md5: String,
    /// Set once the MD5 matched, so a pause during extraction does not hash
    /// 28 GB again.
    verified: bool,
}

/// Shared between the commands and the worker task.
pub struct Downloader {
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    config_dir: PathBuf,
}

impl Downloader {
    pub fn new(config_dir: PathBuf) -> Self {
        let saved = load_saved(&config_dir);
        let mut st = Status::idle(saved.dir.clone());
        // Something was in flight when the launcher last closed: show it as
        // paused with its real progress, so the tab offers "Continue".
        if !saved.dir.is_empty() {
            let part = part_path(Path::new(&saved.dir));
            if let Ok(meta) = std::fs::metadata(&part) {
                st.phase = Phase::Paused;
                st.done = meta.len();
                st.total = saved.total;
                st.message = "Paused -- press Continue to resume where it stopped.".into();
            }
        }
        Downloader {
            status: Arc::new(Mutex::new(st)),
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            config_dir,
        }
    }

    pub fn status(&self) -> Status {
        let mut st = self.status.lock().expect("download status").clone();
        if !st.dir.is_empty() {
            st.free_bytes = free_space(Path::new(&st.dir));
        }
        st
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

// --------------------------------------------------------------- commands ---

/// Start, or continue, a download into `dir`.
pub fn start(app: &AppHandle, dl: &Downloader, dir: String) -> Result<()> {
    if dl.running.swap(true, Ordering::SeqCst) {
        return Err("A download is already running.".into());
    }
    let dir_path = PathBuf::from(dir.trim());
    if dir.trim().is_empty() {
        dl.running.store(false, Ordering::SeqCst);
        return Err("Choose a folder to install the game into first.".into());
    }
    if let Err(e) = std::fs::create_dir_all(dir_path.join(TEMP_DIR)) {
        dl.running.store(false, Ordering::SeqCst);
        return Err(LauncherError::Message(format!("Cannot use that folder: {e}")));
    }

    // Switching folders abandons the old partial file's bookkeeping (the file
    // itself is left where it is -- the player may still want it).
    let mut saved = load_saved(&dl.config_dir);
    if saved.dir != dir_path.to_string_lossy() {
        saved = Saved { dir: dir_path.to_string_lossy().into_owned(), ..Default::default() };
    }
    let _ = store_saved(&dl.config_dir, &saved);

    dl.stop.store(false, Ordering::SeqCst);
    {
        let mut st = dl.status.lock().expect("download status");
        *st = Status::idle(saved.dir.clone());
        st.phase = Phase::Checking;
        st.message = "Asking archive.org for the file...".into();
    }

    let ctx = Ctx {
        app: app.clone(),
        status: dl.status.clone(),
        stop: dl.stop.clone(),
        config_dir: dl.config_dir.clone(),
        dir: dir_path,
    };
    let running = dl.running.clone();
    tauri::async_runtime::spawn(async move {
        let _awake = KeepAwake::new();
        let outcome = run(&ctx, saved).await;
        match outcome {
            Ok(()) => {}
            Err(Stop::Paused) => ctx.set(|s| {
                s.phase = Phase::Paused;
                s.speed = 0.0;
                s.eta_secs = None;
                s.message = "Paused -- press Continue to resume where it stopped.".into();
            }),
            Err(Stop::Failed(msg)) => ctx.set(|s| {
                s.phase = Phase::Failed;
                s.speed = 0.0;
                s.eta_secs = None;
                s.message = msg;
            }),
        }
        running.store(false, Ordering::SeqCst);
        ctx.emit();
    });
    Ok(())
}

/// Pause: the worker notices within one chunk and ends as `Paused`.
pub fn pause(dl: &Downloader) {
    if dl.is_running() {
        dl.stop.store(true, Ordering::SeqCst);
    }
}

/// Stop and delete everything this download wrote (the partial archive and
/// the temp folder). An already extracted game is not touched.
pub fn cancel(app: &AppHandle, dl: &Downloader) -> Result<()> {
    dl.stop.store(true, Ordering::SeqCst);
    // Give the worker a moment to close the file before deleting it; Windows
    // refuses to delete a file that is still open.
    for _ in 0..40 {
        if !dl.is_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let saved = load_saved(&dl.config_dir);
    if !saved.dir.is_empty() {
        let tmp = Path::new(&saved.dir).join(TEMP_DIR);
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
    }
    let _ = std::fs::remove_file(dl.config_dir.join(STATE_FILE));
    {
        let mut st = dl.status.lock().expect("download status");
        *st = Status::idle(saved.dir);
        st.message = "Cancelled -- the partial download was deleted.".into();
    }
    let _ = app.emit("download:status", dl.status());
    Ok(())
}

/// Uninstall: delete the game from `dir`, the Game folder. Only what the
/// archive puts there goes (see `remove_game`), so a Game folder that also
/// holds other files -- or is a whole drive -- keeps them. The folder setting
/// stays: Download puts the game back in the same place.
pub fn uninstall(app: &AppHandle, dl: &Downloader, dir: &str) -> Result<()> {
    if dl.is_running() {
        return Err("A download is running. Cancel it before uninstalling.".into());
    }
    let dir = dir.trim();
    if dir.is_empty() || !crate::game::detect(dir).installed {
        return Err(LauncherError::Message(format!("There is no game to uninstall in {dir}.")));
    }
    remove_game(Path::new(dir)).map_err(|e| {
        LauncherError::Message(format!("Could not delete every game file ({e}). Close anything using them and press Uninstall again."))
    })?;
    {
        let mut st = dl.status.lock().expect("download status");
        *st = Status::idle(dir.to_string());
        st.message = "Uninstalled -- the game was deleted.".into();
    }
    let _ = app.emit("download:status", dl.status());
    Ok(())
}

/// Deletes the game's own entries from `root`: the two game folders, the
/// DepotDownloader leftovers, then the exe -- last, so a failure halfway (a
/// locked file) still shows the game as installed and Uninstall can be pressed
/// again. The folder itself goes only if that leaves it empty. `remove_dir_all`
/// deletes a link, never what it points to.
fn remove_game(root: &Path) -> std::io::Result<()> {
    for name in ["BravoHotelGame", "Engine", ".DepotDownloader"] {
        let path = root.join(name);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        }
    }
    let exe = root.join(crate::game::GAME_EXE);
    if exe.exists() {
        std::fs::remove_file(&exe)?;
    }
    let _ = std::fs::remove_dir(root);
    Ok(())
}

// ----------------------------------------------------------------- worker ---

enum Stop {
    Paused,
    Failed(String),
}

impl<E: std::fmt::Display> From<E> for Stop {
    fn from(e: E) -> Self {
        Stop::Failed(e.to_string())
    }
}

struct Ctx {
    app: AppHandle,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    config_dir: PathBuf,
    dir: PathBuf,
}

impl Ctx {
    fn set(&self, f: impl FnOnce(&mut Status)) {
        let mut st = self.status.lock().expect("download status");
        f(&mut st);
    }
    fn emit(&self) {
        let mut st = self.status.lock().expect("download status").clone();
        st.free_bytes = free_space(&self.dir);
        let _ = self.app.emit("download:status", st);
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

async fn run(ctx: &Ctx, mut saved: Saved) -> std::result::Result<(), Stop> {
    // ---- already there -------------------------------------------------
    // The game is already in this folder (a copy the player had, or an
    // earlier install): nothing to fetch. Not while a download of its own is
    // waiting here to be finished -- a half-unpacked archive can look like an
    // install.
    if !saved.verified && !part_path(&ctx.dir).exists() {
        if let Some(root) = find_install_root(&ctx.dir) {
            finish(ctx, &root, "The game is already in this folder: nothing to download.");
            return Ok(());
        }
    }

    // ---- checking ------------------------------------------------------
    ctx.emit();
    let client = reqwest::Client::builder()
        .user_agent(concat!("SP-Launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()?;

    match fetch_meta(&client).await {
        Ok((size, md5)) => {
            // A different size/hash than last time means the item changed:
            // the old partial file belongs to a different archive.
            if saved.total != 0 && (saved.total != size || (!saved.md5.is_empty() && saved.md5 != md5)) {
                let _ = std::fs::remove_file(part_path(&ctx.dir));
                saved.verified = false;
            }
            saved.total = size;
            saved.md5 = md5;
            let _ = store_saved(&ctx.config_dir, &saved);
        }
        // Offline check, but a size remembered from last time: carry on and
        // let the download itself fail if the network is really gone.
        Err(e) if saved.total > 0 => {
            ctx.set(|s| s.message = format!("Could not refresh file info ({e}); continuing."));
        }
        Err(e) => return Err(Stop::Failed(format!("Could not reach archive.org: {e}"))),
    }

    let part = part_path(&ctx.dir);
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let unpacked_guess = (saved.total as f64 * UNPACKED_ESTIMATE) as u64;
    let needed = saved.total.saturating_sub(have) + unpacked_guess + SPACE_MARGIN;
    let free = free_space(&ctx.dir);
    ctx.set(|s| {
        s.needed_bytes = Some(saved.total + unpacked_guess);
        s.free_bytes = free;
    });
    if let Some(free) = free {
        if free < needed {
            return Err(Stop::Failed(format!(
                "Not enough space on that drive: {} free, about {} needed (download + unpacked game). \
                 Free some space or choose another drive.",
                gb(free),
                gb(needed)
            )));
        }
    }

    // ---- downloading ---------------------------------------------------
    if !saved.verified {
        download(ctx, &client, &part, saved.total).await?;

        // ---- verifying -------------------------------------------------
        if !saved.md5.is_empty() {
            verify(ctx, &part, saved.total, &saved.md5).await?;
        }
        saved.verified = true;
        let _ = store_saved(&ctx.config_dir, &saved);
    }

    // ---- extracting ----------------------------------------------------
    let root = extract(ctx, &part, saved.total).await?;

    // ---- done ----------------------------------------------------------
    finish(ctx, &root, "Installed. The archive was deleted to free the space again.");
    Ok(())
}

/// Point the launcher at a finished install and report it done. The frontend
/// re-reads the config on `download:installed`, so its debounced writer cannot
/// put the old folder back.
fn finish(ctx: &Ctx, root: &Path, message: &str) {
    let root_str = root.to_string_lossy().into_owned();
    if let Some(state) = ctx.app.try_state::<crate::AppState>() {
        let mut cfg = state.config.lock().expect("config mutex");
        cfg.install_dir = root_str.clone();
        let _ = crate::config::save(&state.config_dir, &cfg);
    }
    let _ = std::fs::remove_dir_all(ctx.dir.join(TEMP_DIR));
    let _ = std::fs::remove_file(ctx.config_dir.join(STATE_FILE));
    ctx.set(|s| {
        s.phase = Phase::Done;
        s.speed = 0.0;
        s.eta_secs = None;
        s.done = s.total;
        s.install_dir = root_str.clone();
        s.message = message.into();
    });
    let _ = ctx.app.emit("download:installed", root_str);
}

#[derive(Deserialize)]
struct MetaFiles {
    result: Vec<MetaFile>,
}
#[derive(Deserialize)]
struct MetaFile {
    name: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    md5: Option<String>,
}

/// (size, md5) of `FILE_NAME` from archive.org's metadata API.
async fn fetch_meta(client: &reqwest::Client) -> Result<(u64, String)> {
    let resp = client.get(META_URL).timeout(Duration::from_secs(30)).send().await?.error_for_status()?;
    let files: MetaFiles = resp.json().await?;
    let f = files
        .result
        .into_iter()
        .find(|f| f.name == FILE_NAME)
        .ok_or_else(|| LauncherError::Message(format!("{FILE_NAME} is no longer listed in the archive.org item {ITEM_ID}.")))?;
    let size = f
        .size
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| LauncherError::Message("archive.org did not report the file size.".into()))?;
    Ok((size, f.md5.unwrap_or_default().to_ascii_lowercase()))
}

async fn download(ctx: &Ctx, client: &reqwest::Client, part: &Path, total: u64) -> std::result::Result<(), Stop> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let mut retries = 0u32;
    let mut meter = Meter::new();
    loop {
        if ctx.stopped() {
            return Err(Stop::Paused);
        }
        let mut have = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
        if have > total {
            // Longer than the real file: not ours to trust.
            tokio::fs::remove_file(part).await?;
            have = 0;
        }
        if have == total {
            return Ok(());
        }
        ctx.set(|s| {
            s.phase = Phase::Downloading;
            s.done = have;
            s.total = total;
            s.retries = retries;
            if retries == 0 {
                s.message = if have > 0 { "Resuming...".into() } else { "Downloading...".into() };
            }
        });
        ctx.emit();

        let attempt: std::result::Result<(), String> = async {
            let resp = client
                .get(FILE_URL)
                .header(reqwest::header::RANGE, format!("bytes={have}-"))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT {
                tokio::fs::OpenOptions::new().create(true).append(true).open(part).await.map_err(|e| e.to_string())?
            } else if status.is_success() {
                // The server ignored the range: start over instead of
                // appending a second copy of the beginning.
                have = 0;
                tokio::fs::File::create(part).await.map_err(|e| e.to_string())?
            } else if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                return Ok(()); // nothing left to send; the length check above decides
            } else {
                return Err(format!("archive.org answered {status}"));
            };
            let mut stream = resp.bytes_stream();
            let mut last_emit = Instant::now();
            let mut last_flush = Instant::now();
            loop {
                if ctx.stopped() {
                    file.flush().await.map_err(|e| e.to_string())?;
                    return Ok(());
                }
                let next = tokio::time::timeout(STALL_TIMEOUT, stream.next())
                    .await
                    .map_err(|_| "the connection stalled".to_string())?;
                let Some(chunk) = next else { break };
                let chunk = chunk.map_err(|e| e.to_string())?;
                file.write_all(&chunk).await.map_err(|e| format!("writing to disk failed: {e}"))?;
                have += chunk.len() as u64;
                meter.add(chunk.len() as u64);
                if last_flush.elapsed() > Duration::from_secs(5) {
                    // So a crash loses seconds, not minutes, of progress.
                    file.flush().await.map_err(|e| e.to_string())?;
                    last_flush = Instant::now();
                }
                if last_emit.elapsed() >= EMIT_EVERY {
                    last_emit = Instant::now();
                    let speed = meter.rate();
                    ctx.set(|s| {
                        s.done = have;
                        s.speed = speed;
                        s.eta_secs = eta(total.saturating_sub(have), speed);
                        s.message.clear();
                    });
                    ctx.emit();
                }
            }
            file.flush().await.map_err(|e| e.to_string())?;
            Ok(())
        }
        .await;

        match attempt {
            Ok(()) if ctx.stopped() => return Err(Stop::Paused),
            Ok(()) => {
                let now = tokio::fs::metadata(part).await.map(|m| m.len()).unwrap_or(0);
                if now == total {
                    return Ok(());
                }
                // Stream ended early without an error: retry like any failure.
                if !backoff(ctx, &mut retries, "the connection closed early".into()).await {
                    return Err(Stop::Failed(retry_exhausted()));
                }
            }
            Err(e) => {
                if !backoff(ctx, &mut retries, e).await {
                    return Err(Stop::Failed(retry_exhausted()));
                }
            }
        }
        meter = Meter::new();
    }
}

fn retry_exhausted() -> String {
    format!("The download kept failing ({MAX_RETRIES} tries). Your progress is kept -- press Continue to try again later.")
}

/// Waits before the next try; false once the retry budget is spent. Returns
/// early (true) if the player pauses meanwhile -- the caller then sees the
/// stop flag.
async fn backoff(ctx: &Ctx, retries: &mut u32, why: String) -> bool {
    *retries += 1;
    if *retries > MAX_RETRIES {
        return false;
    }
    let wait = (2u64 << (*retries - 1).min(4)).min(30);
    for left in (1..=wait).rev() {
        if ctx.stopped() {
            return true;
        }
        ctx.set(|s| {
            s.speed = 0.0;
            s.eta_secs = None;
            s.retries = *retries;
            s.message = format!("{why} -- retrying in {left} s (try {}/{MAX_RETRIES})", *retries);
        });
        ctx.emit();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    true
}

async fn verify(ctx: &Ctx, part: &Path, total: u64, expected: &str) -> std::result::Result<(), Stop> {
    ctx.set(|s| {
        s.phase = Phase::Verifying;
        s.done = 0;
        s.total = total;
        s.speed = 0.0;
        s.message = "Checking the download against archive.org's checksum...".into();
    });
    ctx.emit();

    let status = ctx.status.clone();
    let stop = ctx.stop.clone();
    let path = part.to_path_buf();
    let app = ctx.app.clone();
    let dir = ctx.dir.clone();
    let digest = tauri::async_runtime::spawn_blocking(move || -> std::result::Result<Option<String>, String> {
        use md5::{Digest, Md5};
        let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut hasher = Md5::new();
        let mut buf = vec![0u8; 8 * 1024 * 1024];
        let mut done = 0u64;
        let mut meter = Meter::new();
        let mut last = Instant::now();
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let n = f.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            done += n as u64;
            meter.add(n as u64);
            if last.elapsed() >= EMIT_EVERY {
                last = Instant::now();
                let speed = meter.rate();
                let mut st = status.lock().expect("download status");
                st.done = done;
                st.speed = speed;
                st.eta_secs = eta(total.saturating_sub(done), speed);
                let mut snap = st.clone();
                drop(st);
                snap.free_bytes = free_space(&dir);
                let _ = app.emit("download:status", snap);
            }
        }
        Ok(Some(hex(&hasher.finalize())))
    })
    .await
    .map_err(|e| Stop::Failed(e.to_string()))?
    .map_err(Stop::Failed)?;

    match digest {
        None => Err(Stop::Paused),
        Some(d) if d == expected => Ok(()),
        Some(d) => {
            let _ = std::fs::remove_file(part);
            Err(Stop::Failed(format!(
                "The download is damaged (checksum {d}, expected {expected}) and was deleted. Press Download to fetch it again."
            )))
        }
    }
}

// ------------------------------------------------------------- extraction ---

/// 7-Zip compiled into the launcher (see build.rs / resources/README.md).
#[cfg(has_7za)]
const SEVEN_ZIP: &[u8] = include_bytes!("../resources/7za.exe");
#[cfg(not(has_7za))]
const SEVEN_ZIP: &[u8] = &[];

/// The 7-Zip executable to use: the embedded one (written once to the config
/// dir), else a normal 7-Zip installation.
fn seven_zip(config_dir: &Path) -> Option<PathBuf> {
    if !SEVEN_ZIP.is_empty() {
        let path = config_dir.join("tools").join("7za.exe");
        let current = std::fs::read(&path).map(|b| b == SEVEN_ZIP).unwrap_or(false);
        if current {
            return Some(path);
        }
        if std::fs::create_dir_all(path.parent()?).is_ok() && std::fs::write(&path, SEVEN_ZIP).is_ok() {
            return Some(path);
        }
    }
    for base in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(pf) = std::env::var(base) {
            let p = Path::new(&pf).join("7-Zip").join("7z.exe");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn command(exe: &Path) -> std::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut c = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Total unpacked size, from the summary line of `7z l`.
fn unpacked_size(exe: &Path, archive: &Path) -> Option<u64> {
    let out = command(exe).arg("l").arg("-bd").arg(archive).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // The summary is the last line that starts with a date column or blanks
    // and has "files" in it:  "2022-11-14 10:00:00   61234567890  29712345678  1234 files, 56 folders"
    let line = text.lines().rev().find(|l| l.contains(" files"))?;
    line.split_whitespace().find_map(|tok| tok.parse::<u64>().ok().filter(|n| *n > 1024))
}

async fn extract(ctx: &Ctx, archive: &Path, archive_size: u64) -> std::result::Result<PathBuf, Stop> {
    let exe = seven_zip(&ctx.config_dir).ok_or_else(|| {
        Stop::Failed(
            "No 7-Zip found to unpack the game. Install 7-Zip from 7-zip.org and press Continue -- the download is kept."
                .into(),
        )
    })?;

    ctx.set(|s| {
        s.phase = Phase::Extracting;
        s.done = 0;
        s.total = 0;
        s.speed = 0.0;
        s.eta_secs = None;
        s.message = "Reading the archive...".into();
    });
    ctx.emit();

    // Exact space check now that the real unpacked size can be read.
    let exe_l = exe.clone();
    let arch_l = archive.to_path_buf();
    let unpacked = tauri::async_runtime::spawn_blocking(move || unpacked_size(&exe_l, &arch_l))
        .await
        .ok()
        .flatten()
        .unwrap_or((archive_size as f64 * UNPACKED_ESTIMATE) as u64);
    ctx.set(|s| {
        s.total = unpacked;
        s.needed_bytes = Some(archive_size + unpacked);
    });
    if let Some(free) = free_space(&ctx.dir) {
        if free < unpacked + SPACE_MARGIN {
            return Err(Stop::Failed(format!(
                "Not enough space to unpack: {} free, {} needed. Free some space and press Continue -- the download is kept.",
                gb(free),
                gb(unpacked + SPACE_MARGIN)
            )));
        }
    }

    let status = ctx.status.clone();
    let stop = ctx.stop.clone();
    let app = ctx.app.clone();
    let dir = ctx.dir.clone();
    let arch = archive.to_path_buf();
    let result = tauri::async_runtime::spawn_blocking(move || -> std::result::Result<bool, String> {
        // -bsp1: progress to stdout; -bso0: no file list; -aoa: overwrite, so a
        // re-run after an interrupted extraction just completes it.
        let mut child = command(&exe)
            .arg("x")
            .arg(&arch)
            .arg(format!("-o{}", dir.display()))
            .args(["-y", "-aoa", "-bso0", "-bsp1", "-bse2"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start 7-Zip: {e}"))?;
        let mut out = child.stdout.take().ok_or("7-Zip gave no output")?;
        let started = Instant::now();
        let mut buf = [0u8; 4096];
        let mut pending = String::new();
        let mut last = Instant::now();
        loop {
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
            let n = out.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            // 7-Zip redraws its progress line with backspaces / CR.
            let mut pct = None;
            for piece in pending.split(|c| c == '\r' || c == '\n' || c == '\u{8}') {
                if let Some(p) = piece.trim_start().split('%').next().and_then(|s| s.trim().parse::<u64>().ok()) {
                    if p <= 100 && piece.contains('%') {
                        pct = Some(p);
                    }
                }
            }
            if let Some(tail) = pending.rfind(|c| c == '\r' || c == '\n' || c == '\u{8}') {
                pending = pending[tail + 1..].to_string();
            }
            if let Some(p) = pct {
                if last.elapsed() >= EMIT_EVERY {
                    last = Instant::now();
                    let mut st = status.lock().expect("download status");
                    let total = st.total.max(1);
                    let done = total * p / 100;
                    let secs = started.elapsed().as_secs_f64().max(1.0);
                    let speed = done as f64 / secs;
                    st.done = done;
                    st.speed = speed;
                    st.eta_secs = eta(total.saturating_sub(done), speed);
                    st.message = "Unpacking...".into();
                    let mut snap = st.clone();
                    drop(st);
                    snap.free_bytes = free_space(&dir);
                    let _ = app.emit("download:status", snap);
                }
            }
        }
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut err);
        }
        let code = child.wait().map_err(|e| e.to_string())?;
        if !code.success() {
            let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            return Err(format!("7-Zip failed (exit {}): {first}", code.code().unwrap_or(-1)));
        }
        Ok(true)
    })
    .await
    .map_err(|e| Stop::Failed(e.to_string()))?
    .map_err(|e| Stop::Failed(format!("{e}. The download is kept -- press Continue to try unpacking again.")))?;

    if !result {
        return Err(Stop::Paused);
    }

    let root = find_install_root(&ctx.dir).ok_or_else(|| {
        Stop::Failed(format!(
            "Unpacked, but no {} with BravoHotelGame and Engine folders was found under {}. \
             Set the install folder by hand in Settings.",
            crate::game::GAME_EXE,
            ctx.dir.display()
        ))
    })?;
    Ok(flatten_into(&root, &ctx.dir))
}

/// The archive wraps the game in `Manifest #2065353802481281242\` and carries
/// a `.DepotDownloader\` bookkeeping folder. Move the game up into the folder
/// the player chose, so it lands exactly where they asked, and drop the
/// DepotDownloader leftovers.
///
/// Renames only (same volume, instant, no copy). Anything that cannot be moved
/// -- a name that already exists in the target, a locked file -- leaves the
/// game where it is and returns that folder instead: an untidy install that
/// works beats a half-moved one. Returns the folder the game is in afterwards.
fn flatten_into(root: &Path, target: &Path) -> PathBuf {
    let _ = std::fs::remove_dir_all(root.join(".DepotDownloader"));
    if root == target || !root.starts_with(target) {
        return root.to_path_buf();
    }
    let entries: Vec<PathBuf> = match std::fs::read_dir(root) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => return root.to_path_buf(),
    };
    // All or nothing: refuse if any name is already taken in the target.
    if entries.iter().any(|p| p.file_name().map_or(true, |n| target.join(n).exists())) {
        return root.to_path_buf();
    }
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for from in &entries {
        let to = target.join(from.file_name().expect("checked above"));
        if std::fs::rename(from, &to).is_err() {
            // Put back what already moved, keep the original layout.
            for (orig, now) in moved.iter().rev() {
                let _ = std::fs::rename(now, orig);
            }
            return root.to_path_buf();
        }
        moved.push((from.clone(), to));
    }
    // The now-empty wrapper folder(s) between target and root.
    let mut dir = root.to_path_buf();
    while dir != target && std::fs::remove_dir(&dir).is_ok() {
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
    target.to_path_buf()
}

/// Where the game already is in `dir` (or a few folders below it), for the
/// Download tab: a folder that has it needs no download.
pub fn find_game(dir: &str) -> Option<String> {
    let path = Path::new(dir.trim());
    if dir.trim().is_empty() || !path.is_dir() {
        return None;
    }
    find_install_root(path).map(|p| p.to_string_lossy().into_owned())
}

/// The folder that holds the game (the archive may wrap it in one or two
/// folders of its own). Breadth-first, four levels deep, skipping our temp dir,
/// and at most MAX_SCAN folders: a whole drive typed into the folder box must
/// not turn into minutes of disk crawling.
fn find_install_root(dir: &Path) -> Option<PathBuf> {
    const MAX_SCAN: usize = 4000;
    let mut seen = 0;
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..5 {
        let mut next = Vec::new();
        for d in level {
            seen += 1;
            if seen > MAX_SCAN {
                return None;
            }
            if crate::game::detect(&d.to_string_lossy()).installed {
                return Some(d);
            }
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() && e.file_name() != TEMP_DIR {
                        next.push(p);
                    }
                }
            }
        }
        level = next;
    }
    None
}

// ---------------------------------------------------------------- helpers ---

fn part_path(dir: &Path) -> PathBuf {
    dir.join(TEMP_DIR).join(format!("{FILE_NAME}.part"))
}

fn load_saved(config_dir: &Path) -> Saved {
    std::fs::read_to_string(config_dir.join(STATE_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn store_saved(config_dir: &Path, s: &Saved) -> std::io::Result<()> {
    let path = config_dir.join(STATE_FILE);
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(serde_json::to_string_pretty(s).unwrap_or_default().as_bytes())?;
    drop(f);
    std::fs::rename(tmp, path)
}

fn eta(left: u64, speed: f64) -> Option<u64> {
    if speed < 1.0 {
        return None;
    }
    Some((left as f64 / speed).ceil() as u64)
}

fn gb(n: u64) -> String {
    format!("{:.1} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Speed over a sliding window of ~5 s, so the number is readable instead of
/// jumping with every chunk.
struct Meter {
    samples: std::collections::VecDeque<(Instant, u64)>,
    total: u64,
}

impl Meter {
    fn new() -> Self {
        Meter { samples: std::collections::VecDeque::new(), total: 0 }
    }
    fn add(&mut self, n: u64) {
        self.total += n;
        let now = Instant::now();
        self.samples.push_back((now, self.total));
        while let Some(&(t, _)) = self.samples.front() {
            if now.duration_since(t) > Duration::from_secs(5) && self.samples.len() > 2 {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }
    fn rate(&self) -> f64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, b0)), Some(&(t1, b1))) if t1 > t0 => (b1 - b0) as f64 / t1.duration_since(t0).as_secs_f64(),
            _ => 0.0,
        }
    }
}

/// Free bytes for the drive holding `dir` (walks up to an existing parent).
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = dir.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut avail: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointer
    // is a valid u64; the two optional outputs are null.
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok != 0 { Some(avail) } else { None }
}
#[cfg(not(windows))]
pub fn free_space(_dir: &Path) -> Option<u64> {
    None
}

/// Keeps Windows from sleeping while it lives. The flag is per thread, so a
/// small thread holds it for the lifetime of the guard.
struct KeepAwake {
    stop: Arc<AtomicBool>,
}

impl KeepAwake {
    fn new() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(windows)]
        {
            let s = stop.clone();
            std::thread::spawn(move || {
                use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
                // SAFETY: plain flag call, no pointers.
                unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
                while !s.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(500));
                }
                // SAFETY: as above; clears the request.
                unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
            });
        }
        KeepAwake { stop }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta_needs_a_speed() {
        assert_eq!(eta(1000, 0.0), None);
        assert_eq!(eta(1000, 100.0), Some(10));
    }

    #[test]
    fn the_url_is_the_encoded_file_name() {
        assert!(FILE_URL.ends_with(&FILE_NAME.replace(' ', "%20").replace('#', "%23")));
    }

    #[test]
    fn a_saved_state_without_fields_still_loads() {
        let s: Saved = serde_json::from_str("{}").unwrap();
        assert!(s.dir.is_empty() && !s.verified);
    }

    #[test]
    fn the_wrapper_folder_is_flattened_and_depot_leftovers_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Manifest #2065353802481281242");
        std::fs::create_dir_all(root.join("BravoHotelGame")).unwrap();
        std::fs::create_dir_all(root.join("Engine")).unwrap();
        std::fs::create_dir_all(root.join(".DepotDownloader")).unwrap();
        std::fs::write(root.join(crate::game::GAME_EXE), b"x").unwrap();
        let got = flatten_into(&root, tmp.path());
        assert_eq!(got, tmp.path());
        assert!(crate::game::detect(&tmp.path().to_string_lossy()).installed);
        assert!(!root.exists());
        assert!(!tmp.path().join(".DepotDownloader").exists());
    }

    #[test]
    fn a_name_clash_leaves_the_game_where_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wrap");
        std::fs::create_dir_all(root.join("Engine")).unwrap();
        std::fs::create_dir_all(tmp.path().join("Engine")).unwrap();
        assert_eq!(flatten_into(&root, tmp.path()), root);
        assert!(root.join("Engine").exists());
    }

    #[test]
    fn finds_the_game_one_folder_down() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("SUPER PEOPLE");
        std::fs::create_dir_all(root.join("BravoHotelGame")).unwrap();
        std::fs::create_dir_all(root.join("Engine")).unwrap();
        std::fs::write(root.join(crate::game::GAME_EXE), b"x").unwrap();
        assert_eq!(find_install_root(tmp.path()), Some(root));
    }

    #[test]
    fn find_game_answers_for_the_download_tab() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        assert_eq!(find_game(""), None, "no folder");
        assert_eq!(find_game(&format!("{dir}/nowhere")), None, "a folder that doesn't exist");
        assert_eq!(find_game(&dir), None, "an empty folder");

        let root = tmp.path().join("SUPER PEOPLE");
        std::fs::create_dir_all(root.join("BravoHotelGame")).unwrap();
        std::fs::create_dir_all(root.join("Engine")).unwrap();
        std::fs::write(root.join(crate::game::GAME_EXE), b"x").unwrap();
        assert_eq!(find_game(&format!("  {dir}  ")), Some(root.to_string_lossy().into_owned()));
    }

    #[test]
    fn the_scan_gives_up_on_a_huge_folder() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..4100 {
            std::fs::create_dir(tmp.path().join(format!("d{i}"))).unwrap();
        }
        // The game sits past the limit: not found, rather than a long crawl.
        let deep = tmp.path().join("d4099").join("SUPER PEOPLE");
        std::fs::create_dir_all(deep.join("BravoHotelGame")).unwrap();
        std::fs::create_dir_all(deep.join("Engine")).unwrap();
        std::fs::write(deep.join(crate::game::GAME_EXE), b"x").unwrap();
        assert_eq!(find_install_root(tmp.path()), None);
    }

    fn fake_game(root: &Path) {
        std::fs::create_dir_all(root.join("BravoHotelGame/Content/Paks")).unwrap();
        std::fs::create_dir_all(root.join("Engine/Binaries")).unwrap();
        std::fs::create_dir_all(root.join(".DepotDownloader")).unwrap();
        std::fs::write(root.join("BravoHotelGame/Content/Paks/a.pak"), b"x").unwrap();
        std::fs::write(root.join(crate::game::GAME_EXE), b"x").unwrap();
    }

    #[test]
    fn uninstall_removes_the_game_and_its_empty_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("SUPER PEOPLE");
        fake_game(&root);
        remove_game(&root).unwrap();
        assert!(!root.exists());
    }

    #[test]
    fn uninstall_keeps_what_is_not_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_game(root);
        std::fs::create_dir_all(root.join("Screenshots")).unwrap();
        std::fs::write(root.join("notes.txt"), b"mine").unwrap();
        std::fs::create_dir_all(root.join(TEMP_DIR)).unwrap();
        remove_game(root).unwrap();
        for gone in ["BravoHotelGame", "Engine", ".DepotDownloader", crate::game::GAME_EXE] {
            assert!(!root.join(gone).exists(), "{gone} is left");
        }
        assert!(root.join("Screenshots").is_dir());
        assert_eq!(std::fs::read(root.join("notes.txt")).unwrap(), b"mine");
        assert!(root.join(TEMP_DIR).is_dir(), "a paused download is not the game");
        assert!(!crate::game::detect(&root.to_string_lossy()).installed);
    }
}
