//! The Download tab: fetches the game file by file from the team's storage,
//! checks every file against the list on superpeople.dev, and points the
//! launcher at the result.
//!
//! WHAT IT DOWNLOADS
//! -----------------
//! The game as its own files (BravoHotelClient.exe, BravoHotelGame\, Engine\:
//! about 455 files, 30.7 GB), from the team's private Storj bucket. Which
//! files, their sizes and SHA-256 come from the website
//! (`GET /api/launcher/game`); each file is then downloaded through a
//! 15-minute link the website hands to the signed-in player
//! (`POST /api/launcher/game/link`), which also enforces the download limits.
//! The files are on Storj and the list is on the website: someone who could
//! change the bucket still could not get a changed file past the check.
//!
//! THE PIPELINE
//! ------------
//!   checking     the list; what the folder already has (a file of the right
//!                size is there -- or, for Verify files, of the right SHA-256);
//!                free space for what is missing
//!   downloading  twelve files at a time, each an HTTP GET with `Range` into
//!                `<folder>\.sp-download\<sha256>.part`, hashed as it arrives
//!                and moved into place once it matches. BravoHotelClient.exe
//!                goes in last, so a half-downloaded folder never looks like
//!                an install.
//!   done         install folder set, temp folder deleted
//!
//! PAUSE / RESUME
//! --------------
//! Pause stops the downloads and keeps the `.part` files. Continue asks for the
//! list again, skips the files already in place, and sends a `Range` request
//! from each part's current length; the part's bytes are hashed again first,
//! so the check still covers the whole file. The same happens after a launcher
//! restart or a crash: `download.v1.json` in the config dir remembers the
//! folder, and the parts on disk ARE the progress. A server that answers a
//! range request with a full `200` is detected and that file restarts from
//! zero rather than being corrupted by a second copy.
//!
//! Network errors are retried per file (backoff 2 s .. 30 s, 25 tries); a
//! stalled stream (no byte for 45 s) counts as an error. A file whose checksum
//! does not match is deleted and fetched again.
//!
//! While a download runs, Windows is asked not to go to sleep
//! (`SetThreadExecutionState`); the display may still turn off.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{LauncherError, Result};

// ----------------------------------------------------------------- source ---

/// The website's list of the game's files (sp-website app/api/launcher/game).
const LIST_PATH: &str = "/api/launcher/game";
/// A download link for one file, for the signed-in player (…/game/link).
const LINK_PATH: &str = "/api/launcher/game/link";
/// Files downloaded at the same time. The bucket gives about 9 MB/s per
/// connection; 8 filled a 50 MB/s line in a test, 12 leaves room for faster ones.
const PARALLEL: usize = 12;
/// Headroom on top of the free-space requirement.
const SPACE_MARGIN: u64 = 2 * 1024 * 1024 * 1024;

const STATE_FILE: &str = "download.v1.json";
const TEMP_DIR: &str = ".sp-download";
/// The archive earlier launchers downloaded from archive.org, possibly left
/// half-downloaded in the temp folder: up to 28 GB nothing uses any more.
const OLD_ARCHIVE_PART: &str = "Manifest #2065353802481281242.7z.part";
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
    /// Stopped by the user (or by a launcher restart); the `.part` files stay.
    Paused,
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
    /// Bytes of the missing files downloaded so far.
    pub done: u64,
    /// Size of the missing files. 0 while unknown.
    pub total: u64,
    /// Bytes per second, smoothed. 0 when not transferring.
    pub speed: f64,
    /// Seconds left, if it can be estimated.
    pub eta_secs: Option<u64>,
    /// Human-readable line under the bar ("120 of 455 files", errors, ...).
    pub message: String,
    /// Free space on the target drive, and what the missing files need.
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

/// Survives restarts. The `.part` files are the progress; this only remembers
/// WHERE, and how much the run had to fetch (for the bar after a restart).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    dir: String,
    total: u64,
    /// A Verify files run: Continue carries on checking hashes.
    verify: bool,
}

/// The website's list of the game's files.
#[derive(Debug, Clone, Deserialize)]
struct GameList {
    files: Vec<GameFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct GameFile {
    /// Relative, with `/`: `BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.pak`.
    path: String,
    size: u64,
    /// Lowercase hex.
    sha256: String,
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
        // The 7-Zip earlier launchers unpacked the archive with: unused now.
        let _ = std::fs::remove_file(config_dir.join("tools").join("7za.exe"));
        let saved = load_saved(&config_dir);
        let mut st = Status::idle(saved.dir.clone());
        // Something was in flight when the launcher last closed: show it as
        // paused with its real progress, so the tab offers "Continue".
        if !saved.dir.is_empty() {
            let temp = Path::new(&saved.dir).join(TEMP_DIR);
            let _ = std::fs::remove_file(temp.join(OLD_ARCHIVE_PART));
            if let Some(done) = parts_on_disk(&temp) {
                st.phase = Phase::Paused;
                st.done = done;
                st.total = saved.total.max(done);
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

/// Start, or continue, a download into `dir`. `verify` (Verify files) checks
/// the SHA-256 of the files already there instead of only their size, and
/// downloads the damaged ones again. `session` is the player's Discord
/// sign-in, which the website wants for each file's link.
pub fn start(app: &AppHandle, dl: &Downloader, dir: String, verify: bool, session: String) -> Result<()> {
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

    // Switching folders abandons the old partial files' bookkeeping (the files
    // themselves are left where they are -- the player may still want them).
    let mut saved = load_saved(&dl.config_dir);
    if saved.dir != dir_path.to_string_lossy() {
        saved = Saved { dir: dir_path.to_string_lossy().into_owned(), ..Default::default() };
    }
    let verify = verify || saved.verify;
    saved.verify = verify;
    let _ = store_saved(&dl.config_dir, &saved);

    dl.stop.store(false, Ordering::SeqCst);
    {
        let mut st = dl.status.lock().expect("download status");
        *st = Status::idle(saved.dir.clone());
        st.phase = Phase::Checking;
        st.message = "Getting the list of the game's files...".into();
    }
    let source = Source::Website { session };

    // The worker reports through this; the Tauri calls stay here, out of the
    // worker's code (a test binary cannot load the webview they bring in).
    let events = app.clone();
    let tell: Tell = Arc::new(move |event| match event {
        Event::Status(st) => {
            let _ = events.emit("download:status", st);
        }
        Event::Installed(root) => {
            if let Some(state) = events.try_state::<crate::AppState>() {
                let mut cfg = state.config.lock().expect("config mutex");
                cfg.install_dir = root.clone();
                let _ = crate::config::save(&state.config_dir, &cfg);
            }
            let _ = events.emit("download:installed", root);
        }
    });
    let ctx = Ctx {
        tell,
        status: dl.status.clone(),
        stop: dl.stop.clone(),
        config_dir: dl.config_dir.clone(),
        dir: dir_path,
    };
    let running = dl.running.clone();
    tauri::async_runtime::spawn(async move {
        let _awake = KeepAwake::new();
        let outcome = run(&ctx, &source, saved, verify).await;
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

/// Pause: the downloads notice within one chunk and the run ends as `Paused`.
pub fn pause(dl: &Downloader) {
    if dl.is_running() {
        dl.stop.store(true, Ordering::SeqCst);
    }
}

/// Stop and delete what this download has not finished (the temp folder).
/// Files already checked and moved into place stay: the next download skips
/// them.
pub fn cancel(app: &AppHandle, dl: &Downloader) -> Result<()> {
    dl.stop.store(true, Ordering::SeqCst);
    // Give the downloads a moment to close their files before deleting them;
    // Windows refuses to delete a file that is still open.
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

/// Uninstall: delete the game from `dir`, the Game folder. Only the game's own
/// folders and exe go (see `remove_game`), so a Game folder that also holds
/// other files -- or is a whole drive -- keeps them. The folder setting stays:
/// Download puts the game back in the same place.
pub fn uninstall(app: &AppHandle, dl: &Downloader, dir: &str) -> Result<()> {
    if dl.is_running() {
        return Err("A download is running. Cancel it before uninstalling.".into());
    }
    let dir = dir.trim();
    if dir.is_empty() || !crate::game::detect(dir).installed {
        return Err(LauncherError::Message(format!("There is no game to uninstall in {dir}.")));
    }
    // The uninstall window's bar: files deleted so far, of how many.
    let mut last = Instant::now();
    remove_game(Path::new(dir), &mut |done, total| {
        if done == total || last.elapsed() >= EMIT_EVERY {
            last = Instant::now();
            let _ = app.emit("uninstall:progress", serde_json::json!({ "done": done, "total": total }));
        }
    })
    .map_err(|e| {
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

/// The game's own entries in its folder; the exe goes last.
const GAME_DIRS: [&str; 3] = ["BravoHotelGame", "Engine", ".DepotDownloader"];

/// What the uninstall window says will go: the game's files and their size.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct Footprint {
    pub files: u64,
    pub bytes: u64,
}

pub fn footprint(dir: &str) -> Footprint {
    let root = Path::new(dir.trim());
    let mut found = Footprint::default();
    for path in game_files(root) {
        found.files += 1;
        found.bytes += std::fs::symlink_metadata(&path).map(|m| m.len()).unwrap_or(0);
    }
    found
}

/// Every file of the game in `root`, the exe last. Links are not followed:
/// what one points to is not the game's.
fn game_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs: Vec<PathBuf> = GAME_DIRS.iter().map(|name| root.join(name)).collect();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(t) if t.is_dir() => dirs.push(entry.path()),
                Ok(t) if t.is_file() => files.push(entry.path()),
                _ => {}
            }
        }
    }
    let exe = root.join(crate::game::GAME_EXE);
    if exe.is_file() {
        files.push(exe);
    }
    files
}

/// Deletes the game's own entries from `root`: the files of the two game
/// folders and the DepotDownloader leftovers of the old archive one by one
/// (`progress` hears how many of how many), what is left of those folders,
/// then the exe -- last, so a failure halfway (a locked file) still shows the
/// game as installed and Uninstall can be pressed again. The folder itself
/// goes only if that leaves it empty. `remove_dir_all` deletes a link, never
/// what it points to.
fn remove_game(root: &Path, progress: &mut dyn FnMut(u64, u64)) -> std::io::Result<()> {
    let exe = root.join(crate::game::GAME_EXE);
    let files: Vec<PathBuf> = game_files(root).into_iter().filter(|f| *f != exe).collect();
    let total = files.len() as u64 + u64::from(exe.exists());
    for (i, file) in files.iter().enumerate() {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        progress(i as u64 + 1, total);
    }
    for name in GAME_DIRS {
        let path = root.join(name);
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        }
    }
    if exe.exists() {
        std::fs::remove_file(&exe)?;
        progress(total, total);
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

/// What the worker tells the launcher.
enum Event {
    Status(Status),
    /// The game is in this folder now: it becomes the Game folder.
    Installed(String),
}

type Tell = Arc<dyn Fn(Event) + Send + Sync>;

struct Ctx {
    tell: Tell,
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
        (self.tell)(Event::Status(st));
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

async fn run(ctx: &Ctx, source: &Source, mut saved: Saved, verify: bool) -> std::result::Result<(), Stop> {
    // ---- checking ------------------------------------------------------
    ctx.emit();
    let client = http_client()?;
    let list = fetch_list(&client)
        .await
        .map_err(|e| Stop::Failed(format!("Could not get the list of the game's files from superpeople.dev: {e}")))?;

    let temp = ctx.dir.join(TEMP_DIR);
    let _ = std::fs::remove_file(temp.join(OLD_ARCHIVE_PART));
    // The game already in this folder (or a folder or two below it) is
    // completed where it is; otherwise it goes straight into the folder.
    let root = find_install_root(&ctx.dir).unwrap_or_else(|| ctx.dir.clone());
    let mut jobs = missing(&root, &list.files);
    if verify {
        let damaged = damaged(ctx, &root, &list.files, &jobs).await?;
        jobs.extend(damaged);
        jobs.sort_by_key(|f| (is_exe(f), std::cmp::Reverse(f.size)));
    }
    if jobs.is_empty() {
        let message = if verify {
            format!("All {} files are fine: nothing to download.", list.files.len())
        } else {
            "The game is already in this folder: nothing to download.".to_string()
        };
        finish(ctx, &root, &message);
        return Ok(());
    }

    let total: u64 = jobs.iter().map(|j| j.size).sum();
    let have: u64 = jobs.iter().map(|j| file_len(&part_path(&temp, j)).min(j.size)).sum();
    saved.total = total;
    let _ = store_saved(&ctx.config_dir, &saved);

    let needed = total - have + SPACE_MARGIN;
    let free = free_space(&ctx.dir);
    ctx.set(|s| {
        s.needed_bytes = Some(total);
        s.free_bytes = free;
        s.done = have;
        s.total = total;
    });
    if let Some(free) = free {
        if free < needed {
            return Err(Stop::Failed(format!(
                "Not enough space on that drive: {} free, about {} needed. Free some space or choose another drive.",
                gb(free),
                gb(needed)
            )));
        }
    }

    // ---- downloading ---------------------------------------------------
    fetch_all(ctx, &client, source, &root, &temp, &jobs, have).await?;

    // ---- done ----------------------------------------------------------
    let message = if verify {
        match jobs.len() {
            1 => "Repaired: 1 missing or damaged file was downloaded again.".to_string(),
            n => format!("Repaired: {n} missing or damaged files were downloaded again."),
        }
    } else {
        format!("Installed. All {} files were checked.", list.files.len())
    };
    finish(ctx, &root, &message);
    Ok(())
}

/// Verify files: the files of the right size whose SHA-256 is wrong anyway.
/// Reads the whole game, so it reports progress like a download and stops
/// for Pause.
async fn damaged(ctx: &Ctx, root: &Path, files: &[GameFile], missing: &[GameFile]) -> std::result::Result<Vec<GameFile>, Stop> {
    let skip: std::collections::HashSet<&str> = missing.iter().map(|f| f.path.as_str()).collect();
    let check: Vec<(PathBuf, GameFile)> =
        files.iter().filter(|f| !skip.contains(f.path.as_str())).map(|f| (target_path(root, f), f.clone())).collect();
    let total: u64 = check.iter().map(|(_, f)| f.size).sum();
    ctx.set(|s| {
        s.phase = Phase::Checking;
        s.done = 0;
        s.total = total;
        s.message = format!("Checking files: 0 of {}", check.len());
    });
    ctx.emit();

    let status = ctx.status.clone();
    let stop = ctx.stop.clone();
    let tell = ctx.tell.clone();
    let dir = ctx.dir.clone();
    let found = tokio::task::spawn_blocking(move || -> Option<Vec<GameFile>> {
        let mut bad = Vec::new();
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        let mut done = 0u64;
        let mut meter = Meter::new();
        let mut last = Instant::now();
        for (i, (path, file)) in check.iter().enumerate() {
            let mut hasher = Sha256::new();
            let Ok(mut reader) = std::fs::File::open(path) else {
                bad.push(file.clone());
                continue;
            };
            loop {
                if stop.load(Ordering::SeqCst) {
                    return None;
                }
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => {
                        // Unreadable counts as damaged: it is downloaded again.
                        hasher = Sha256::new();
                        break;
                    }
                };
                hasher.update(&buf[..n]);
                done += n as u64;
                meter.add(n as u64);
                if last.elapsed() >= EMIT_EVERY {
                    last = Instant::now();
                    let speed = meter.rate();
                    let snapshot = {
                        let mut st = status.lock().expect("download status");
                        st.done = done;
                        st.speed = speed;
                        st.eta_secs = eta(total.saturating_sub(done), speed);
                        st.message = format!("Checking files: {} of {}", i + 1, check.len());
                        st.clone()
                    };
                    tell(Event::Status(Status { free_bytes: free_space(&dir), ..snapshot }));
                }
            }
            if hex(&hasher.finalize()) != file.sha256 {
                bad.push(file.clone());
            }
        }
        Some(bad)
    })
    .await
    .map_err(|e| Stop::Failed(e.to_string()))?;
    found.ok_or(Stop::Paused)
}

/// Report a finished install: the launcher makes it the Game folder (`start`'s
/// `tell`). The frontend re-reads the config on `download:installed`, so its
/// debounced writer cannot put the old folder back.
fn finish(ctx: &Ctx, root: &Path, message: &str) {
    let root_str = root.to_string_lossy().into_owned();
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
    (ctx.tell)(Event::Installed(root_str));
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("SP-Launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()?)
}

async fn fetch_list(client: &reqwest::Client) -> Result<GameList> {
    let url = format!("{}{LIST_PATH}", crate::auth::site_url());
    let list: GameList = client.get(url).timeout(Duration::from_secs(30)).send().await?.error_for_status()?.json().await?;
    check_list(&list).map_err(|why| LauncherError::Message(format!("the list is not valid ({why})")))?;
    Ok(list)
}

/// The list decides where files are written and what is downloaded, so it is
/// checked before anything is: paths that stay inside the game folder, real
/// checksums, each file once.
fn check_list(list: &GameList) -> std::result::Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for f in &list.files {
        if !safe_path(&f.path) {
            return Err(format!("bad path {:?}", f.path));
        }
        if f.sha256.len() != 64 || !f.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(format!("bad checksum for {}", f.path));
        }
        // Windows paths ignore case: two entries must not land on one file.
        if !seen.insert(f.path.to_ascii_lowercase()) {
            return Err(format!("{} is listed twice", f.path));
        }
    }
    if !list.files.iter().any(|f| f.path == crate::game::GAME_EXE) {
        return Err(format!("{} is not in it", crate::game::GAME_EXE));
    }
    Ok(())
}

/// A relative path with `/` whose every part is a plain name: nothing that
/// could leave the game folder (`..`, a drive, a leading `/`) or that Windows
/// would read differently (`\`, a trailing dot or space).
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with('.')
                && !part.ends_with(' ')
                && !part.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        })
}

/// The files of the list the folder does not have: missing, or of another
/// size. Biggest first, the exe last.
fn missing(root: &Path, files: &[GameFile]) -> Vec<GameFile> {
    let mut jobs: Vec<GameFile> = files
        .iter()
        .filter(|f| std::fs::metadata(target_path(root, f)).map(|m| m.len() != f.size).unwrap_or(true))
        .cloned()
        .collect();
    jobs.sort_by_key(|f| (is_exe(f), std::cmp::Reverse(f.size)));
    jobs
}

fn is_exe(f: &GameFile) -> bool {
    f.path == crate::game::GAME_EXE
}

fn target_path(root: &Path, f: &GameFile) -> PathBuf {
    f.path.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn part_path(temp: &Path, f: &GameFile) -> PathBuf {
    temp.join(format!("{}.part", f.sha256))
}

/// Where a file's download link comes from.
enum Source {
    /// The website: a 15-minute link per file for the signed-in player, within
    /// the download limits (sp-website app/api/launcher/game/link).
    Website { session: String },
    /// `base` plus the file's path (the tests' local server).
    #[cfg(test)]
    Base(String),
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct LinkAnswer {
    url: Option<String>,
    error: Option<String>,
    /// Unix milliseconds: when a download limit ends.
    until: Option<u64>,
}

/// A link to download `job` from, asked for again at each try: a link only
/// works for 15 minutes, and the website hands out the same one meanwhile.
async fn link(client: &reqwest::Client, source: &Source, job: &GameFile) -> std::result::Result<String, Miss> {
    let session = match source {
        #[cfg(test)]
        Source::Base(base) => return Ok(file_url(base, &job.path)),
        Source::Website { session } => session,
    };
    let resp = client
        .post(format!("{}{LINK_PATH}", crate::auth::site_url()))
        .bearer_auth(session)
        .json(&serde_json::json!({ "path": job.path }))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| Miss::Retry(format!("superpeople.dev could not be reached ({e})")))?;
    let status = resp.status();
    let answer: LinkAnswer = resp.json().await.unwrap_or_default();
    match status.as_u16() {
        200 => answer
            .url
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| Miss::Retry("superpeople.dev sent no link".into())),
        401 => Err(Miss::Fatal(
            "Your Discord sign-in has expired. Sign in again, then press Continue -- your progress is kept.".into(),
        )),
        403 if answer.error.as_deref() == Some("banned") => Err(Miss::Fatal(
            "This account is banned from SUPER PEOPLE, so it cannot download the game. If you think this is a mistake, contact us on Discord."
                .into(),
        )),
        429 => Err(Miss::Fatal(limit_reached(answer.until))),
        404 => Err(Miss::Fatal(format!("{} is no longer in the game's file list. Press Continue to get the new list.", job.path))),
        503 if answer.error.as_deref() == Some("setup") => {
            Err(Miss::Fatal("Game downloads are not open yet. Try again later -- your progress is kept.".into()))
        }
        _ => Err(Miss::Retry(format!("superpeople.dev answered {status}"))),
    }
}

/// The download limit's message: until when, from the website's answer.
fn limit_reached(until_ms: Option<u64>) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let hours = until_ms.map(|until| until.saturating_sub(now).div_ceil(3_600_000)).unwrap_or(24).max(1);
    let wait = if hours == 1 { "about an hour".to_string() } else { format!("about {hours} hours") };
    format!(
        "Downloads are paused for {wait}: this account or internet connection downloaded more than twice the game \
         in the last hour. Your progress is kept. If this is a mistake, ask the team on Discord."
    )
}

/// `base` plus the path, every part percent-encoded.
#[cfg(test)]
fn file_url(base: &str, path: &str) -> String {
    let mut url = base.to_string();
    for (i, part) in path.split('/').enumerate() {
        if i > 0 {
            url.push('/');
        }
        for b in part.bytes() {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                url.push(b as char);
            } else {
                url.push_str(&format!("%{b:02X}"));
            }
        }
    }
    url
}

/// The download's progress, shared by the files downloading at once.
struct Progress {
    done: AtomicU64,
    total: u64,
    files_done: AtomicUsize,
    files: usize,
    meter: Mutex<Meter>,
    last_emit: Mutex<Instant>,
}

impl Progress {
    /// A file's bytes on disk are now `now` (it was `counted`).
    fn count(&self, counted: &mut u64, now: u64) {
        if now >= *counted {
            let more = now - *counted;
            self.done.fetch_add(more, Ordering::SeqCst);
            self.meter.lock().expect("meter").add(more);
        } else {
            self.done.fetch_sub(*counted - now, Ordering::SeqCst);
        }
        *counted = now;
    }

    fn tick(&self, ctx: &Ctx) {
        {
            let mut last = self.last_emit.lock().expect("last emit");
            if last.elapsed() < EMIT_EVERY {
                return;
            }
            *last = Instant::now();
        }
        let done = self.done.load(Ordering::SeqCst);
        let speed = self.meter.lock().expect("meter").rate();
        ctx.set(|s| {
            s.done = done;
            s.speed = speed;
            s.eta_secs = eta(self.total.saturating_sub(done), speed);
        });
        ctx.emit();
    }

    fn file_finished(&self, ctx: &Ctx) {
        let n = self.files_done.fetch_add(1, Ordering::SeqCst) + 1;
        ctx.set(|s| s.message = format!("{n} of {} files", self.files));
    }
}

async fn fetch_all(
    ctx: &Ctx,
    client: &reqwest::Client,
    source: &Source,
    root: &Path,
    temp: &Path,
    jobs: &[GameFile],
    have: u64,
) -> std::result::Result<(), Stop> {
    use futures_util::StreamExt;

    std::fs::create_dir_all(temp)?;
    let total: u64 = jobs.iter().map(|j| j.size).sum();
    let progress = Progress {
        done: AtomicU64::new(have),
        total,
        files_done: AtomicUsize::new(0),
        files: jobs.len(),
        meter: Mutex::new(Meter::new()),
        last_emit: Mutex::new(Instant::now()),
    };
    ctx.set(|s| {
        s.phase = Phase::Downloading;
        s.done = have;
        s.total = total;
        s.message = if have > 0 { "Resuming...".into() } else { format!("0 of {} files", jobs.len()) };
    });
    ctx.emit();

    // Collected first: a lazy `map` closure here would not be Send.
    let tries: Vec<_> = jobs.iter().map(|job| fetch_one(ctx, client, source, temp, job, &progress)).collect();
    let mut results = futures_util::stream::iter(tries).buffer_unordered(PARALLEL);
    let mut paused = false;
    while let Some(result) = results.next().await {
        match result {
            Ok(()) => {}
            Err(Stop::Paused) => paused = true,
            // Dropping the others stops them; their parts are kept.
            Err(failed) => return Err(failed),
        }
    }
    drop(results);
    if paused || ctx.stopped() {
        return Err(Stop::Paused);
    }

    // Everything checked: the files go into place, the exe last.
    ctx.set(|s| {
        s.done = progress.done.load(Ordering::SeqCst);
        s.speed = 0.0;
        s.eta_secs = None;
        s.message = format!("Putting the {} files in place...", jobs.len());
    });
    ctx.emit();
    for job in jobs {
        let part = part_path(temp, job);
        let target = target_path(root, job);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&part, &target).map_err(|e| {
            Stop::Failed(format!("Could not put {} in place ({e}). Close the game and press Continue.", job.path))
        })?;
    }
    Ok(())
}

/// Why a try failed: worth another try, or not (the file is not there at all,
/// the download limit, an expired sign-in).
enum Miss {
    Retry(String),
    Fatal(String),
}

/// One file into its `.part`, checked against its SHA-256. Moving it into
/// place is `fetch_all`'s, once every file is here.
async fn fetch_one(
    ctx: &Ctx,
    client: &reqwest::Client,
    source: &Source,
    temp: &Path,
    job: &GameFile,
    progress: &Progress,
) -> std::result::Result<(), Stop> {
    let part = part_path(temp, job);
    let name = job.path.rsplit('/').next().unwrap_or(&job.path).to_string();
    // What the start-up total already counts for this file.
    let mut counted = file_len(&part).min(job.size);
    // The hash of the part's bytes, kept between tries so a retry does not
    // read them again.
    let mut hashed: Option<(u64, Sha256)> = None;
    let mut retries = 0u32;
    loop {
        if ctx.stopped() {
            return Err(Stop::Paused);
        }
        let mut have = file_len(&part);
        if have > job.size {
            // Longer than the real file: not ours to trust.
            std::fs::remove_file(&part)?;
            have = 0;
        }
        progress.count(&mut counted, have);
        let mut hasher = match hashed.take() {
            Some((n, h)) if n == have => h,
            _ => hash_prefix(&part, have).await?,
        };

        if have < job.size {
            let outcome = match link(client, source, job).await {
                Ok(url) => stream_into(ctx, client, &url, &name, &part, job.size, &mut have, &mut hasher, progress, &mut counted).await,
                Err(miss) => Err(miss),
            };
            match outcome {
                Ok(()) if have == job.size => {}
                Ok(()) if ctx.stopped() => return Err(Stop::Paused),
                Ok(()) => {
                    hashed = Some((have, hasher));
                    if !backoff(ctx, &mut retries, format!("{name}: the connection closed early")).await {
                        return Err(Stop::Failed(retry_exhausted()));
                    }
                    continue;
                }
                Err(Miss::Fatal(why)) => return Err(Stop::Failed(why)),
                Err(Miss::Retry(why)) => {
                    if ctx.stopped() {
                        return Err(Stop::Paused);
                    }
                    hashed = Some((have, hasher));
                    if !backoff(ctx, &mut retries, format!("{name}: {why}")).await {
                        return Err(Stop::Failed(retry_exhausted()));
                    }
                    continue;
                }
            }
        }

        // The whole file is here: check it.
        if hex(&hasher.finalize()) != job.sha256 {
            std::fs::remove_file(&part)?;
            progress.count(&mut counted, 0);
            if !backoff(ctx, &mut retries, format!("{name} arrived damaged")).await {
                return Err(Stop::Failed(format!(
                    "{} kept arriving damaged. Press Continue to try again later.",
                    job.path
                )));
            }
            continue;
        }
        progress.file_finished(ctx);
        progress.tick(ctx);
        return Ok(());
    }
}

/// One request for the rest of the file, appended to the part and hashed as
/// it arrives. `have` and `hasher` stay right whatever happens, so the next
/// try carries on from them.
#[allow(clippy::too_many_arguments)]
async fn stream_into(
    ctx: &Ctx,
    client: &reqwest::Client,
    url: &str,
    name: &str,
    part: &Path,
    size: u64,
    have: &mut u64,
    hasher: &mut Sha256,
    progress: &Progress,
    counted: &mut u64,
) -> std::result::Result<(), Miss> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;

    let resp = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={have}-"))
        .send()
        .await
        .map_err(|e| Miss::Retry(e.to_string()))?;
    let status = resp.status();
    let mut file = if status == reqwest::StatusCode::PARTIAL_CONTENT {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(part)
            .await
            .map_err(|e| Miss::Retry(e.to_string()))?
    } else if status.is_success() {
        // The server ignored the range: start over instead of appending a
        // second copy of the beginning.
        *have = 0;
        *hasher = Sha256::new();
        progress.count(counted, 0);
        tokio::fs::File::create(part).await.map_err(|e| Miss::Retry(e.to_string()))?
    } else if matches!(status.as_u16(), 404 | 410) {
        return Err(Miss::Fatal(format!("The download server does not have {name} ({status}). Tell the team on Discord.")));
    } else {
        // 403 included: an expired link. The next try asks for a new one.
        return Err(Miss::Retry(format!("the server answered {status}")));
    };

    let mut stream = resp.bytes_stream();
    let mut last_flush = Instant::now();
    loop {
        if ctx.stopped() {
            break;
        }
        let next = tokio::time::timeout(STALL_TIMEOUT, stream.next())
            .await
            .map_err(|_| Miss::Retry("the connection stalled".into()))?;
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|e| Miss::Retry(e.to_string()))?;
        if *have + chunk.len() as u64 > size {
            return Err(Miss::Retry("the server sent more than the file's size".into()));
        }
        file.write_all(&chunk)
            .await
            .map_err(|e| Miss::Fatal(format!("Writing to disk failed: {e}. Your progress is kept -- press Continue.")))?;
        hasher.update(&chunk);
        *have += chunk.len() as u64;
        progress.count(counted, *have);
        progress.tick(ctx);
        if last_flush.elapsed() > Duration::from_secs(5) {
            // So a crash loses seconds, not minutes, of progress.
            file.flush().await.map_err(|e| Miss::Retry(e.to_string()))?;
            last_flush = Instant::now();
        }
    }
    file.flush().await.map_err(|e| Miss::Retry(e.to_string()))?;
    Ok(())
}

/// The SHA-256 state after the first `len` bytes of a part (a resumed file is
/// checked whole). Off the async threads: a part can be most of a gigabyte.
async fn hash_prefix(part: &Path, len: u64) -> std::result::Result<Sha256, Stop> {
    if len == 0 {
        return Ok(Sha256::new());
    }
    let path = part.to_path_buf();
    // Tokio's own: the download already runs on the Tokio runtime Tauri starts.
    tokio::task::spawn_blocking(move || -> std::io::Result<Sha256> {
        let mut hasher = Sha256::new();
        let mut file = std::fs::File::open(&path)?.take(len);
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher)
    })
    .await
    .map_err(|e| Stop::Failed(e.to_string()))?
    .map_err(|e| Stop::Failed(e.to_string()))
}

fn retry_exhausted() -> String {
    format!("The download kept failing ({MAX_RETRIES} tries). Your progress is kept -- press Continue to try again later.")
}

/// Waits before a file's next try; false once its retry budget is spent.
/// Returns early (true) if the player pauses meanwhile -- the caller then sees
/// the stop flag. The other files keep downloading meanwhile.
async fn backoff(ctx: &Ctx, retries: &mut u32, why: String) -> bool {
    *retries += 1;
    if *retries > MAX_RETRIES {
        return false;
    }
    let wait = (2u64 << (*retries - 1).min(4)).min(30);
    ctx.set(|s| {
        s.retries = s.retries.max(*retries);
        s.message = format!("{why} -- retrying in {wait} s (try {}/{MAX_RETRIES})", *retries);
    });
    ctx.emit();
    for _ in 0..wait {
        if ctx.stopped() {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    true
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

/// The folder that holds the game (a copy may sit in a folder or two of its
/// own). Breadth-first, four levels deep, skipping our temp dir, and at most
/// MAX_SCAN folders: a whole drive typed into the folder box must not turn
/// into minutes of disk crawling.
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

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The bytes of the downloads waiting in `temp` (`<sha256>.part` files), or
/// None when there are none.
fn parts_on_disk(temp: &Path) -> Option<u64> {
    let mut found = None;
    for entry in std::fs::read_dir(temp).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_part = name
            .strip_suffix(".part")
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
        if is_part {
            *found.get_or_insert(0) += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    found
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

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn entry(path: &str, body: &[u8]) -> GameFile {
        GameFile { path: path.into(), size: body.len() as u64, sha256: sha(body) }
    }

    fn list(files: Vec<GameFile>) -> GameList {
        GameList { files }
    }

    #[test]
    fn eta_needs_a_speed() {
        assert_eq!(eta(1000, 0.0), None);
        assert_eq!(eta(1000, 100.0), Some(10));
    }

    #[test]
    fn a_saved_state_without_fields_still_loads() {
        let s: Saved = serde_json::from_str("{}").unwrap();
        assert!(s.dir.is_empty() && s.total == 0);
        // The archive.org launcher's state file still loads.
        let old: Saved = serde_json::from_str(r#"{"dir":"D:\\SP","total":5,"md5":"ab","verified":true}"#).unwrap();
        assert_eq!(old.dir, "D:\\SP");
    }

    #[test]
    fn paths_stay_inside_the_game_folder() {
        for ok in ["BravoHotelClient.exe", "BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.pak", "Engine/a b/c.dll"] {
            assert!(safe_path(ok), "{ok}");
        }
        for bad in ["", "/etc/x", "../x", "a/../../x", "a//b", "C:/x", "a\\b", "a/./b", "a/b.", "a/b ", "a/b?"] {
            assert!(!safe_path(bad), "{bad}");
        }
    }

    #[test]
    fn the_list_is_checked_before_use() {
        let exe = entry(crate::game::GAME_EXE, b"exe");
        assert!(check_list(&list(vec![exe.clone()])).is_ok());

        let mut bad_hash = entry("a.pak", b"a");
        bad_hash.sha256 = "ABC".into();
        assert!(check_list(&list(vec![exe.clone(), bad_hash])).is_err(), "bad checksum");

        let twice = list(vec![exe.clone(), entry("A.pak", b"a"), entry("a.PAK", b"b")]);
        assert!(check_list(&twice).is_err(), "one file twice (Windows ignores case)");

        assert!(check_list(&list(vec![entry("a.pak", b"a")])).is_err(), "no exe");
        assert!(check_list(&list(vec![exe, entry("../evil.dll", b"x")])).is_err(), "path out of the folder");
    }

    #[test]
    fn urls_are_encoded_part_by_part() {
        assert_eq!(
            file_url("https://h/raw/k/b/game/", "Engine/Some Dir/a#1.pak"),
            "https://h/raw/k/b/game/Engine/Some%20Dir/a%231.pak"
        );
    }

    #[test]
    fn only_missing_files_are_fetched_biggest_first_exe_last() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("BravoHotelGame")).unwrap();
        std::fs::write(root.join("BravoHotelGame/here.pak"), b"12345").unwrap();
        std::fs::write(root.join("BravoHotelGame/short.pak"), b"12").unwrap();
        let files = vec![
            entry(crate::game::GAME_EXE, b"exe-exe-exe-exe"),
            entry("BravoHotelGame/here.pak", b"12345"),
            entry("BravoHotelGame/short.pak", b"12345"),
            entry("BravoHotelGame/new.pak", b"1234567"),
        ];
        let jobs: Vec<String> = missing(root, &files).into_iter().map(|f| f.path).collect();
        assert_eq!(jobs, ["BravoHotelGame/new.pak", "BravoHotelGame/short.pak", crate::game::GAME_EXE]);
    }

    #[test]
    fn only_our_parts_count_as_progress() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(parts_on_disk(tmp.path()), None);
        std::fs::write(tmp.path().join(OLD_ARCHIVE_PART), b"old archive").unwrap();
        assert_eq!(parts_on_disk(tmp.path()), None);
        std::fs::write(tmp.path().join(format!("{}.part", "a".repeat(64))), b"1234").unwrap();
        assert_eq!(parts_on_disk(tmp.path()), Some(4));
    }

    // ---- a real download from a small local server ----------------------

    /// Serves `files` over plain HTTP with `Range`, like the bucket. The first
    /// answer for `cut` stops halfway, like a dropped connection.
    async fn serve(files: Vec<(String, Vec<u8>)>, cut: Option<String>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cut = Arc::new(Mutex::new(cut));
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let files = files.clone();
                let cut = cut.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").trim_start_matches("/game/").to_string();
                    let Some((_, body)) = files.iter().find(|(p, _)| file_url("", p) == path) else {
                        let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        return;
                    };
                    let from: usize = req
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|r| r.trim_end_matches('-').to_string()))
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0);
                    let rest = &body[from..];
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{}\r\nConnection: close\r\n\r\n",
                        rest.len(),
                        body.len() - 1,
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let drop_now = cut.lock().unwrap().as_deref() == Some(path.as_str());
                    if drop_now {
                        *cut.lock().unwrap() = None;
                        let _ = sock.write_all(&rest[..rest.len() / 2]).await;
                        return; // connection closes halfway
                    }
                    let _ = sock.write_all(rest).await;
                });
            }
        });
        format!("http://{addr}/game/")
    }

    fn test_ctx(dir: &Path) -> Ctx {
        Ctx {
            tell: Arc::new(|_| {}),
            status: Arc::new(Mutex::new(Status::idle(dir.to_string_lossy().into_owned()))),
            stop: Arc::new(AtomicBool::new(false)),
            config_dir: dir.join("config"),
            dir: dir.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn downloads_resumes_checks_and_places_every_file() {
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let bodies = vec![
            (crate::game::GAME_EXE.to_string(), b"MZ the exe".to_vec()),
            ("BravoHotelGame/Content/Paks/big one.pak".to_string(), big.clone()),
            ("Engine/Binaries/x.dll".to_string(), b"dll".to_vec()),
        ];
        let base = serve(bodies.clone(), Some(file_url("", "BravoHotelGame/Content/Paks/big one.pak"))).await;
        let files: Vec<GameFile> = bodies.iter().map(|(p, b)| entry(p, b)).collect();

        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let temp = tmp.path().join(TEMP_DIR);
        // Half of the big file was already downloaded by an earlier run.
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(part_path(&temp, &files[1]), &big[..1000]).unwrap();

        let jobs = missing(tmp.path(), &files);
        let client = http_client().unwrap();
        let result = fetch_all(&ctx, &client, &Source::Base(base.clone()), tmp.path(), &temp, &jobs, 1000).await;
        assert!(result.is_ok(), "{:?}", result.err().map(|e| match e {
            Stop::Failed(m) => m,
            Stop::Paused => "paused".into(),
        }));
        for (path, body) in &bodies {
            assert_eq!(&std::fs::read(tmp.path().join(path)).unwrap(), body, "{path}");
        }
        assert!(crate::game::detect(&tmp.path().to_string_lossy()).installed);
        let st = ctx.status.lock().unwrap().clone();
        assert_eq!(st.done, st.total);
    }

    #[tokio::test]
    async fn a_damaged_file_is_never_put_in_place() {
        let bodies = vec![("BravoHotelGame/a.pak".to_string(), b"the real bytes".to_vec())];
        let base = serve(bodies, None).await;
        let mut wrong = entry("BravoHotelGame/a.pak", b"the real bytes");
        wrong.sha256 = sha(b"other bytes");

        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let temp = tmp.path().join(TEMP_DIR);
        // Pause after the first failed check instead of waiting out 25 tries.
        let stop = ctx.stop.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            stop.store(true, Ordering::SeqCst);
        });
        let client = http_client().unwrap();
        let result = fetch_all(&ctx, &client, &Source::Base(base.clone()), tmp.path(), &temp, &[wrong], 0).await;
        assert!(matches!(result, Err(Stop::Paused)));
        assert!(!tmp.path().join("BravoHotelGame/a.pak").exists());
        assert!(ctx.status.lock().unwrap().message.contains("arrived damaged"));
    }

    #[tokio::test]
    async fn a_file_the_server_does_not_have_fails_at_once() {
        let base = serve(vec![], None).await;
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let client = http_client().unwrap();
        let job = entry("BravoHotelGame/gone.pak", b"x");
        let result = fetch_all(&ctx, &client, &Source::Base(base.clone()), tmp.path(), &tmp.path().join(TEMP_DIR), &[job], 0).await;
        assert!(matches!(result, Err(Stop::Failed(m)) if m.contains("does not have")));
    }

    #[tokio::test]
    async fn verify_finds_damaged_files_of_the_right_size() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("BravoHotelGame")).unwrap();
        std::fs::write(tmp.path().join("BravoHotelGame/good.pak"), b"good bytes").unwrap();
        std::fs::write(tmp.path().join("BravoHotelGame/bad.pak"), b"BAD  bytes").unwrap();
        let files = vec![
            entry("BravoHotelGame/good.pak", b"good bytes"),
            entry("BravoHotelGame/bad.pak", b"good bytes"),
            entry("BravoHotelGame/gone.pak", b"not here"),
        ];
        let missing_now = missing(tmp.path(), &files);
        assert_eq!(missing_now.len(), 1, "only the absent file is missing by size");
        let ctx = test_ctx(tmp.path());
        let damaged: Vec<String> =
            damaged(&ctx, tmp.path(), &files, &missing_now).await.ok().unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(damaged, ["BravoHotelGame/bad.pak"]);
    }

    #[test]
    fn the_limit_message_says_how_long() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        assert!(limit_reached(Some(now + 23 * 3_600_000 + 60_000)).contains("about 24 hours"));
        assert!(limit_reached(Some(now + 10 * 60_000)).contains("about an hour"));
        assert!(limit_reached(None).contains("about 24 hours"));
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
        assert_eq!(footprint(&root.to_string_lossy()), Footprint { files: 2, bytes: 2 });
        let mut seen = Vec::new();
        remove_game(&root, &mut |done, total| seen.push((done, total))).unwrap();
        assert!(!root.exists());
        assert_eq!(seen, [(1, 2), (2, 2)], "each file, the exe last");
    }

    #[test]
    fn uninstall_keeps_what_is_not_the_game() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fake_game(root);
        std::fs::create_dir_all(root.join("Screenshots")).unwrap();
        std::fs::write(root.join("notes.txt"), b"mine").unwrap();
        std::fs::create_dir_all(root.join(TEMP_DIR)).unwrap();
        remove_game(root, &mut |_, _| {}).unwrap();
        for gone in ["BravoHotelGame", "Engine", ".DepotDownloader", crate::game::GAME_EXE] {
            assert!(!root.join(gone).exists(), "{gone} is left");
        }
        assert!(root.join("Screenshots").is_dir());
        assert_eq!(std::fs::read(root.join("notes.txt")).unwrap(), b"mine");
        assert!(root.join(TEMP_DIR).is_dir(), "a paused download is not the game");
        assert!(!crate::game::detect(&root.to_string_lossy()).installed);
    }
}
