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
//! Verify files remembers the files it found fine (`Saved::fine`: size, time
//! and expected SHA-256), so a run that stopped carries on from there.
//!
//! Every file a run hashes and finds to be the official one (downloaded, or
//! found fine by Verify files) is also remembered for Play (integrity.rs), and
//! Verify files moves extra paks and DLLs out of the game's way.
//!
//! Updating the launcher in the middle of a download or Verify files pauses
//! it first (`pause_for_update`, files closed), and the updated launcher
//! continues it by itself once the player is signed in (`take_resume`).
//!
//! Network errors are retried per file (backoff 2 s .. 30 s, 25 tries); a
//! stalled stream (no byte for 45 s) counts as an error. A file whose checksum
//! does not match is deleted and fetched again.
//!
//! THE BACKUP
//! ----------
//! When the storage cannot serve -- its monthly limit reached, blocked where
//! the player is, down -- the launcher quietly turns to the backup copy: the
//! same game in one 7z on archive.org (the list says where, `backup`). Up to
//! 2 GB still missing (a repair), those files come one by one, unpacked by
//! archive.org; more, and the whole archive comes down and 7-Zip unpacks only
//! what is missing. Either way every file is checked against the same list.
//! The website can send launchers there at once (`backup.only`). It turns to
//! the backup after STORAGE_TRIES tries in a row that brought nothing, on any
//! file. Nothing on screen says which source it was.
//!
//! The archive comes down in 64 MB pieces (`backup-<size>.7z.001`, `.002`,
//! ...: a split archive, which 7-Zip opens as one), eight at a time, spread
//! over every archive.org server that holds the item (its metadata,
//! `workable_servers`). archive.org's own link sends everyone to one server
//! over one connection: 9 MB/s in a test, where its other server gave 29 MB/s
//! and eight connections over both filled the 50 MB/s line. Each piece
//! resumes where it stopped; a server that fails hands its piece to the next.
//!
//! THE RACE
//! --------
//! Where the player is, one source can be much faster than the other. With
//! more than 2 GB to fetch and the room for the archive, the backup gets a
//! few seconds first (its pieces are kept), then the storage starts; once the
//! storage has run a few seconds too, the backup takes over if it would finish
//! clearly sooner, unpacking included. If the chosen backup then fails, the
//! storage carries on: files already checked are never fetched twice.
//!
//! The team's #launcher-logs hears of a new download once it is known where
//! the files come from: at once without a race, else when it is judged, with
//! both speeds ("download.started" and its `source`). The other source taking
//! over later is "download.switched".
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
use crate::integrity;

// ----------------------------------------------------------------- source ---

/// The website's list of the game's files (sp-website app/api/launcher/game).
const LIST_PATH: &str = "/api/launcher/game";
/// A download link for one file, for the signed-in player (…/game/link).
const LINK_PATH: &str = "/api/launcher/game/link";
/// Waits before asking for the list again: three tries in all. A connection
/// that drops while the list arrives reads as "error decoding response body"
/// (05.10.2026, a player whose download never started), and without the list
/// nothing can be downloaded.
const LIST_WAITS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(6)];
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
/// Tries in a row that bring nothing from the storage before turning to the
/// backup.
const STORAGE_TRIES: u32 = 4;
/// Up to this much still missing, the backup sends single files; more, and the
/// whole archive is downloaded and unpacked here.
const BACKUP_SINGLE_FILES: u64 = 2 * 1024 * 1024 * 1024;
/// Single files from the backup at once: archive.org unpacks each on its side.
const BACKUP_PARALLEL: usize = 4;
/// archive.org can take a while to start sending a file it unpacks.
const BACKUP_STALL: Duration = Duration::from_secs(120);
/// The backup archive's pieces: at least this big, at most 999 of them
/// (three-digit names).
const PIECE: u64 = if cfg!(test) { 64 * 1024 } else { 64 * 1024 * 1024 };
/// Pieces downloaded at once, over all of archive.org's servers. One
/// connection gave 2 to 29 MB/s in a test; 4 to 16 over both servers, 45 to
/// 52 MB/s (the line's limit).
const ARCHIVE_PARALLEL: usize = 8;
/// The whole archive in one file, as the previous launcher downloaded it.
const WHOLE_ARCHIVE: &str = "game.7z.part";
/// Where the backup archive is unpacked, in the temp folder.
const UNPACK_DIR: &str = "unpacked";
/// The race (see the top): how long the backup is tried, then how long the
/// storage runs before the two are compared.
const PROBE_BACKUP: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(7) };
const PROBE_STORAGE: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(8) };
/// 7-Zip's unpacking, counted in the backup's time. The game barely
/// compresses, so it runs at disk speed (1.8 GB/s on an SSD in a test); this
/// is a slow hard drive's.
const UNPACK_SPEED: f64 = 150e6;
/// The backup takes over only if it would finish in under this share of the
/// storage's time: it needs the room for the archive, and the unpacking.
const RACE_MARGIN: f64 = 0.8;
/// A retry countdown's step (shorter in tests).
const TICK: Duration = if cfg!(test) { Duration::from_millis(10) } else { Duration::from_secs(1) };
const EMIT_EVERY: Duration = Duration::from_millis(250);

// ------------------------------------------------------------------ types ---

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nothing started, or a previous run was cancelled.
    Idle,
    Checking,
    Downloading,
    /// Unpacking the backup archive (only when the storage could not serve).
    Preparing,
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
    /// The launcher closed itself for its own update in the middle of this
    /// run (`pause_for_update`): the next start continues it without a click.
    resume: bool,
    /// Verify files: the files this run already found fine, so a run that
    /// stopped carries on from there instead of reading the whole game again.
    fine: Vec<FineFile>,
}

/// A file Verify files found fine, as it was then: a different size, time or
/// expected checksum and it is checked again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct FineFile {
    path: String,
    size: u64,
    /// Last write, in milliseconds since 1970.
    modified: u64,
    sha256: String,
}

/// A file's size and last write time, as `FineFile` keeps them.
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some((meta.len(), modified))
}

/// The website's list of the game's files.
#[derive(Debug, Clone, Deserialize)]
struct GameList {
    files: Vec<GameFile>,
    /// The same game in one archive elsewhere, for when the storage cannot
    /// serve. Absent from older answers: no backup then.
    #[serde(default)]
    backup: Option<Backup>,
}

/// The backup copy (sp-website lib/downloads.ts): the game in one 7z on
/// archive.org, under `folder` inside it.
#[derive(Debug, Clone, Deserialize)]
struct Backup {
    url: String,
    /// The game's folder inside the archive, with its trailing `/`.
    folder: String,
    size: u64,
    /// The storage is switched off: go to the backup at once.
    #[serde(default)]
    only: bool,
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
    let source = Source::Website { session: session.clone() };

    // The worker reports through this; the Tauri calls stay here, out of the
    // worker's code (a test binary cannot load the webview they bring in).
    let events = app.clone();
    let tell: Tell = Arc::new(move |event| match event {
        Event::Status(st) => {
            let _ = events.emit("download:status", st);
        }
        // #launcher-logs, through the website (auth::report).
        Event::Report(event) => crate::auth::report(session.clone(), event),
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
        start: Mutex::default(),
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
            Err(stop @ (Stop::Failed(_) | Stop::Backup | Stop::Faster)) => {
                let msg = match stop {
                    Stop::Failed(msg) => msg,
                    _ => "The download could not be finished. Press Continue to try again -- your progress is kept.".to_string(),
                };
                (ctx.tell)(Event::Report(serde_json::json!({ "action": "download.failed", "reason": msg })));
                ctx.set(|s| {
                    s.phase = Phase::Failed;
                    s.speed = 0.0;
                    s.eta_secs = None;
                    s.message = msg;
                });
            }
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

/// The launcher is about to close for its own update: a running download or
/// Verify files stops where it is, its files closed, and the updated launcher
/// continues it (`take_resume`). True if one was running.
pub fn pause_for_update(dl: &Downloader) -> bool {
    if !dl.is_running() {
        return false;
    }
    dl.stop.store(true, Ordering::SeqCst);
    // A download stops within a chunk, Verify files within a 4 MB read,
    // 7-Zip as soon as it is killed.
    for _ in 0..100 {
        if !dl.is_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut saved = load_saved(&dl.config_dir);
    if saved.dir.is_empty() {
        return false;
    }
    saved.resume = true;
    store_saved(&dl.config_dir, &saved).is_ok()
}

/// What `pause_for_update` stopped, once: its folder, and whether it was
/// Verify files.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Resume {
    pub dir: String,
    pub verify: bool,
}

/// The run an update interrupted, if any, and forget it: it continues once.
pub fn take_resume(dl: &Downloader) -> Option<Resume> {
    if dl.is_running() {
        return None;
    }
    let mut saved = load_saved(&dl.config_dir);
    if !saved.resume || saved.dir.is_empty() {
        return None;
    }
    saved.resume = false;
    let _ = store_saved(&dl.config_dir, &saved);
    Some(Resume { dir: saved.dir, verify: saved.verify })
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
pub fn uninstall(app: &AppHandle, dl: &Downloader, dir: &str) -> Result<Footprint> {
    if dl.is_running() {
        return Err("A download is running. Cancel it before uninstalling.".into());
    }
    let dir = dir.trim();
    if dir.is_empty() || !crate::game::detect(dir).installed {
        return Err(LauncherError::Message(format!("There is no game to uninstall in {dir}.")));
    }
    let freed = footprint(dir);
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
    Ok(freed)
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
    /// The storage cannot serve: turn to the backup (never leaves `run`).
    Backup,
    /// The race: the backup is faster here (never leaves `run`).
    Faster,
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
    /// For the team's #launcher-logs (`{"action": …}`, see auth::report).
    Report(serde_json::Value),
}

type Tell = Arc<dyn Fn(Event) + Send + Sync>;

struct Ctx {
    tell: Tell,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    config_dir: PathBuf,
    dir: PathBuf,
    /// A new download's start, told once it is known where the files come
    /// from (`came_from`).
    start: Mutex<Start>,
}

/// "download.started" waits for the race (see the top): told before it, it
/// could not say whether the storage or the backup sends the game.
#[derive(Default)]
struct Start {
    /// A new download (not a verify or a resume): files and bytes to fetch.
    due: Option<(usize, u64)>,
    /// Where the files come from, once told.
    from: Option<&'static str>,
}

impl Ctx {
    /// For #launcher-logs: where a new download's files come from ("storage"
    /// or "backup"). "download.started" the first time, with the race's
    /// speeds when there was one; "download.switched" and why when the other
    /// source takes over later. Nothing for a verify or a resume.
    fn came_from(&self, from: &'static str, speeds: Option<serde_json::Value>, why: &str) {
        let mut start = self.start.lock().expect("download start");
        let Some((files, bytes)) = start.due else { return };
        if start.from == Some(from) {
            return;
        }
        let report = match start.from {
            None => serde_json::json!({ "action": "download.started", "files": files, "bytes": bytes, "source": from, "speeds": speeds }),
            Some(_) => serde_json::json!({ "action": "download.switched", "source": from, "reason": why }),
        };
        start.from = Some(from);
        drop(start);
        (self.tell)(Event::Report(report));
    }
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
    let mut list = fetch_list()
        .await
        .map_err(|e| Stop::Failed(with_stall_hint(format!("Could not get the list of the game's files from superpeople.dev: {e}"))))?;
    // The startup pictures are the launcher's own (startup_images.rs, written
    // by `finish`): never fetched, never "repaired" back to the original.
    list.files.retain(|f| !crate::startup_images::is_ours(&f.path));

    let temp = ctx.dir.join(TEMP_DIR);
    let _ = std::fs::remove_file(temp.join(OLD_ARCHIVE_PART));
    // The game already in this folder (or a folder or two below it) is
    // completed where it is; otherwise it goes straight into the folder.
    let root = find_install_root(&ctx.dir).unwrap_or_else(|| ctx.dir.clone());
    // A DLSS / XeSS library the player swapped for a build the launcher recognises stays as their
    // tool put it (upscalers.rs): not fetched again, not checked as a game file.
    let kept = kept_swaps(&root, &list.files);
    list.files.retain(|f| !kept.iter().any(|(path, _)| *path == f.path));
    let kept_note = match kept.len() {
        0 => String::new(),
        _ => format!(" Kept your {}.", kept.iter().map(|(_, what)| what.as_str()).collect::<Vec<_>>().join(", ")),
    };
    let mut jobs = missing(&root, &list.files);
    // Verify files: paks and DLLs that are not part of the game go out of its
    // way first (integrity.rs), so Play accepts the folder afterwards.
    let moved = if verify {
        integrity::move_extras(&root, &official(&list))
            .map_err(|e| Stop::Failed(format!("Could not move the files that are not part of the game ({e}). Close the game and try again.")))?
    } else {
        Vec::new()
    };
    let moved_note = match moved.len() {
        0 => String::new(),
        1 => format!(" 1 file that is not part of the game was moved to {} in the game folder.", integrity::REMOVED_DIR),
        n => format!(" {n} files that are not part of the game were moved to {} in the game folder.", integrity::REMOVED_DIR),
    };
    if verify {
        let damaged = damaged(ctx, &root, &list.files, &jobs, &mut saved).await?;
        // The files read and found fine: Play trusts them while they stay so.
        let fine = saved.fine.iter().map(|f| integrity::Checked { path: f.path.clone(), size: f.size, modified: f.modified, sha256: f.sha256.clone() });
        if let Err(e) = integrity::remember(&ctx.config_dir, &root, fine) {
            eprintln!("[integrity] {e}");
        }
        jobs.extend(damaged);
        jobs.sort_by_key(|f| (is_exe(f), std::cmp::Reverse(f.size)));
    }
    if jobs.is_empty() {
        if verify {
            (ctx.tell)(Event::Report(serde_json::json!({ "action": "verify.ok", "files": list.files.len(), "moved": moved.len() })));
        }
        let message = if verify {
            format!("All {} files are fine: nothing to download.{moved_note}{kept_note}", list.files.len())
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
            return Err(Stop::Failed(short_of_space(free, needed, "", " or choose another drive.")));
        }
    }

    // ---- downloading ---------------------------------------------------
    if !verify && have == 0 {
        ctx.start.lock().expect("download start").due = Some((jobs.len(), total));
    }
    let started = Instant::now();
    let backup = list.backup.as_ref();
    let only_backup = backup.is_some_and(|b| b.only);
    // The race's first half: the backup's few seconds (see the top).
    let race = match backup {
        Some(b) if !b.only => backup_time(ctx, &client, b, &temp, total - have).await?,
        _ => None,
    };
    // Without a race the source is known now; with one, once it is judged
    // (Progress::judge).
    if race.is_none() {
        ctx.came_from(if only_backup { "backup" } else { "storage" }, None, "");
    }
    let storage = if only_backup {
        Err(Stop::Backup)
    } else {
        fetch_parts(ctx, &client, source, &temp, &jobs, have, backup.is_some(), race.as_ref()).await
    };
    let unavailable = || Stop::Failed("The game's download server cannot be reached. Try again later -- your progress is kept.".into());
    let from = match storage {
        Ok(()) => "storage",
        Err(Stop::Faster) => {
            let backup = backup.ok_or_else(unavailable)?;
            match from_backup(ctx, &client, backup, &temp, &jobs, 0).await {
                Ok(()) => "backup",
                // The backup failed after all: the storage, which was
                // working, carries on with what is still missing.
                Err(Stop::Failed(_)) => {
                    ctx.came_from("storage", None, "The backup failed: the storage carries on.");
                    let have = jobs.iter().map(|j| file_len(&part_path(&temp, j)).min(j.size)).sum();
                    fetch_parts(ctx, &client, source, &temp, &jobs, have, false, None).await?;
                    "storage"
                }
                Err(stop) => return Err(stop),
            }
        }
        Err(Stop::Backup) => {
            let backup = backup.ok_or_else(unavailable)?;
            ctx.came_from("backup", None, "The storage could not send the files.");
            from_backup(ctx, &client, backup, &temp, &jobs, BACKUP_SINGLE_FILES).await?;
            "backup"
        }
        Err(stop) => return Err(stop),
    };
    // The storage sent it all before the race was judged.
    ctx.came_from(from, race.as_ref().and_then(Race::speeds), "");
    place_all(ctx, &root, &temp, &jobs)?;
    // Each was hashed as it arrived and placed only if it matched the list.
    let placed = jobs.iter().filter_map(|j| integrity::checked(&root, &j.path, &j.sha256));
    if let Err(e) = integrity::remember(&ctx.config_dir, &root, placed) {
        eprintln!("[integrity] {e}");
    }
    let speeds = race.as_ref().and_then(Race::speeds);
    (ctx.tell)(Event::Report(if verify {
        serde_json::json!({ "action": "verify.repaired", "files": jobs.len(), "bytes": total, "source": from, "speeds": speeds, "moved": moved.len() })
    } else {
        // How long it took, when this run did the whole download.
        let seconds = (have == 0).then(|| started.elapsed().as_secs());
        serde_json::json!({ "action": "download.finished", "files": jobs.len(), "bytes": total, "seconds": seconds, "source": from, "speeds": speeds })
    }));

    // ---- done ----------------------------------------------------------
    let message = if verify {
        match jobs.len() {
            1 => format!("Repaired: 1 missing or damaged file was downloaded again.{moved_note}"),
            n => format!("Repaired: {n} missing or damaged files were downloaded again.{moved_note}"),
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
///
/// The files found fine go into `saved.fine` (on disk every few seconds and at
/// a pause), and a run that continues skips those still the same: Pause, a
/// restart or a launcher update cost only the file being read.
async fn damaged(ctx: &Ctx, root: &Path, files: &[GameFile], missing: &[GameFile], saved: &mut Saved) -> std::result::Result<Vec<GameFile>, Stop> {
    let skip: std::collections::HashSet<&str> = missing.iter().map(|f| f.path.as_str()).collect();
    let check: Vec<(PathBuf, GameFile)> =
        files.iter().filter(|f| !skip.contains(f.path.as_str())).map(|f| (target_path(root, f), f.clone())).collect();
    let total: u64 = check.iter().map(|(_, f)| f.size).sum();
    // Still exactly as found fine: same size, time and expected checksum.
    let known: std::collections::HashMap<String, FineFile> = std::mem::take(&mut saved.fine).into_iter().map(|f| (f.path.clone(), f)).collect();
    let still_fine = |path: &Path, file: &GameFile| {
        known.get(&file.path).is_some_and(|f| {
            f.sha256 == file.sha256 && f.size == file.size && stamp(path) == Some((f.size, f.modified))
        })
    };
    let mut fine: Vec<FineFile> = Vec::new();
    let mut todo: Vec<(PathBuf, GameFile)> = Vec::new();
    for (path, file) in check {
        if still_fine(&path, &file) {
            fine.push(known[&file.path].clone());
        } else {
            todo.push((path, file));
        }
    }
    let skipped = fine.len();
    let count = skipped + todo.len();
    let skipped_bytes: u64 = fine.iter().map(|f| f.size).sum();
    saved.fine = fine;
    ctx.set(|s| {
        s.phase = Phase::Checking;
        s.done = skipped_bytes;
        s.total = total;
        s.message = format!("Checking files: {skipped} of {count}");
    });
    ctx.emit();

    let status = ctx.status.clone();
    let stop = ctx.stop.clone();
    let tell = ctx.tell.clone();
    let dir = ctx.dir.clone();
    let config_dir = ctx.config_dir.clone();
    let mut progress = saved.clone();
    let (found, progress) = tokio::task::spawn_blocking(move || -> (Option<Vec<GameFile>>, Saved) {
        let mut bad = Vec::new();
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        let mut done = skipped_bytes;
        let mut meter = Meter::new();
        let mut last = Instant::now();
        let mut last_store = Instant::now();
        for (i, (path, file)) in todo.iter().enumerate() {
            let i = skipped + i;
            let before = stamp(path);
            let mut hasher = Sha256::new();
            let Ok(mut reader) = std::fs::File::open(path) else {
                bad.push(file.clone());
                continue;
            };
            loop {
                if stop.load(Ordering::SeqCst) {
                    let _ = store_saved(&config_dir, &progress);
                    return (None, progress);
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
                        st.message = format!("Checking files: {} of {count}", i + 1);
                        st.clone()
                    };
                    tell(Event::Status(Status { free_bytes: free_space(&dir), ..snapshot }));
                }
            }
            if hex(&hasher.finalize()) != file.sha256 {
                bad.push(file.clone());
            } else if let Some((size, modified)) = before.filter(|b| stamp(path) == Some(*b)) {
                // Unchanged while it was read: remembered as fine.
                progress.fine.push(FineFile { path: file.path.clone(), size, modified, sha256: file.sha256.clone() });
                if last_store.elapsed() >= Duration::from_secs(2) {
                    let _ = store_saved(&config_dir, &progress);
                    last_store = Instant::now();
                }
            }
        }
        let _ = store_saved(&config_dir, &progress);
        (Some(bad), progress)
    })
    .await
    .map_err(|e| Stop::Failed(e.to_string()))?;
    *saved = progress;
    found.ok_or(Stop::Paused)
}

/// Report a finished install: the launcher makes it the Game folder (`start`'s
/// `tell`). The frontend re-reads the config on `download:installed`, so its
/// debounced writer cannot put the old folder back.
fn finish(ctx: &Ctx, root: &Path, message: &str) {
    let root_str = root.to_string_lossy().into_owned();
    if let Err(e) = crate::startup_images::apply(&root_str) {
        eprintln!("[startup images] {e}");
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
    (ctx.tell)(Event::Installed(root_str));
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("SP-Launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        // The game's files come in `Range` parts and are checked byte for
        // byte against their size and SHA-256: never as a compressed answer.
        .no_gzip()
        .build()?)
}

/// The list's own client: it asks for a compressed answer, 74 KB -> 24 KB
/// (05.10.2026), so less of it can be lost on the way.
fn list_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("SP-Launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .gzip(true)
        .build()?)
}

/// The list as integrity.rs compares the folder with it.
fn official(list: &GameList) -> Vec<integrity::Official> {
    list.files.iter().map(|f| integrity::Official { path: f.path.clone(), size: f.size, sha256: f.sha256.clone() }).collect()
}

/// The game's files, as Play checks them (integrity.rs): the website's list,
/// without the launcher's own startup pictures.
pub async fn official_files() -> Result<Vec<integrity::Official>> {
    let mut list = fetch_list().await?;
    list.files.retain(|f| !crate::startup_images::is_ours(&f.path));
    Ok(official(&list))
}

async fn fetch_list() -> Result<GameList> {
    fetch_list_from(&format!("{}{LIST_PATH}", crate::auth::site_url()), &LIST_WAITS).await
}

/// The list from `url`, asked again after each wait in `waits` when the
/// network fails it. An answer from the site that refuses it (4xx) is final,
/// and so is a list that arrives whole but is not valid.
async fn fetch_list_from(url: &str, waits: &[Duration]) -> Result<GameList> {
    let client = list_client()?;
    let mut tries = 0;
    loop {
        tries += 1;
        let got = async {
            client.get(url).timeout(Duration::from_secs(30)).send().await?.error_for_status()?.json::<GameList>().await
        }
        .await;
        match got {
            Ok(list) => {
                check_list(&list).map_err(|why| LauncherError::Message(format!("the list is not valid ({why})")))?;
                return Ok(list);
            }
            Err(e) => {
                let refused = e.status().is_some_and(|s| s.is_client_error());
                if refused || tries > waits.len() {
                    let after = if tries > 1 { format!(" ({tries} tries)") } else { String::new() };
                    return Err(LauncherError::Message(format!("network: {}{after}", with_causes(&e))));
                }
                tokio::time::sleep(waits[tries - 1]).await;
            }
        }
    }
}

/// A network error with what caused it: reqwest's own text is only the
/// first line ("error decoding response body"), its causes say why ("connection
/// reset", "EOF while parsing ..."). Each cause once.
fn with_causes(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut text = e.to_string();
    let mut cause = e.source();
    while let Some(c) = cause {
        let part = c.to_string();
        if !part.is_empty() && !text.contains(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        cause = c.source();
    }
    text
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
    if let Some(backup) = &list.backup {
        let folder = backup.folder.strip_suffix('/').unwrap_or(&backup.folder);
        if !backup.url.starts_with("https://") || (!folder.is_empty() && (!backup.folder.ends_with('/') || !safe_path(folder))) {
            return Err("the backup is not valid".into());
        }
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

/// The DLSS / XeSS libraries in the folder that are the player's recognised swap, not the game's
/// own: (path, "DLSS 310.9.1.0 (nvngx_dlss.dll)"). One of the game's size is left to the usual
/// check, which reads every file anyway.
fn kept_swaps(root: &Path, files: &[GameFile]) -> Vec<(String, String)> {
    files
        .iter()
        .filter_map(|f| {
            let library = crate::upscalers::slot(&f.path)?;
            let path = target_path(root, f);
            let size = std::fs::metadata(&path).ok()?.len();
            if size == f.size {
                return None;
            }
            let version = crate::upscalers::recognise(&path, library)?;
            let file = f.path.rsplit('/').next().unwrap_or(&f.path);
            Some((f.path.clone(), format!("{} {version} ({file})", library.name())))
        })
        .collect()
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
    /// The backup: one file from inside its archive, which archive.org unpacks
    /// on its side (slowly, and from the start each time).
    Archive { url: String, folder: String },
    /// `base` plus the file's path (the tests' local server, as the storage).
    #[cfg(test)]
    Base(String),
}

impl Source {
    /// The storage, which the backup can stand in for.
    fn is_storage(&self) -> bool {
        !matches!(self, Source::Archive { .. })
    }
    fn stall(&self) -> Duration {
        if self.is_storage() { STALL_TIMEOUT } else { BACKUP_STALL }
    }
    fn parallel(&self) -> usize {
        if self.is_storage() { PARALLEL } else { BACKUP_PARALLEL }
    }
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
        Source::Archive { url, folder } => return Ok(format!("{url}/{}", encode_all(&format!("{folder}{}", job.path)))),
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
        // The storage is not set up: the backup, if there is one.
        503 if answer.error.as_deref() == Some("setup") => Err(Miss::Backup),
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

/// Every byte but letters, digits and `-._~` percent-encoded, `/` too: a
/// path inside an archive, as archive.org's own links write it.
fn encode_all(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
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

/// The race between the storage and the backup (see the top).
struct Race {
    /// How long the backup would take for what is left, unpacking included.
    backup_secs: f64,
    /// Bytes per second, from its few seconds.
    backup_speed: f64,
    /// The storage's, once it has run PROBE_STORAGE.
    storage_speed: Mutex<Option<f64>>,
}

impl Race {
    /// Both speeds (bytes per second), once the storage's is measured.
    fn speeds(&self) -> Option<serde_json::Value> {
        let storage = (*self.storage_speed.lock().expect("race"))?;
        Some(serde_json::json!({ "storage": storage.round(), "backup": self.backup_speed.min(1e12).round() }))
    }
}

/// The download's progress, shared by the files downloading at once.
struct Progress<'a> {
    done: AtomicU64,
    total: u64,
    files_done: AtomicUsize,
    files: usize,
    meter: Mutex<Meter>,
    last_emit: Mutex<Instant>,
    /// There is a backup to turn to when the storage cannot serve.
    fallback: bool,
    /// The storage gave up: every file stops and the backup takes over.
    to_backup: AtomicBool,
    /// The race still to judge, and since when the storage has been sending.
    race: Option<&'a Race>,
    race_since: Mutex<Option<Instant>>,
    /// The race went to the backup: it is faster here.
    faster: AtomicBool,
}

impl Progress<'_> {
    /// The race's second half: once the storage has sent for PROBE_STORAGE,
    /// the backup takes over if it would finish in under RACE_MARGIN of the
    /// storage's time. Judged once, and told (`Ctx::came_from`).
    fn judge(&self, ctx: &Ctx, done: u64, speed: f64) {
        let Some(race) = self.race else { return };
        let mut since = self.race_since.lock().expect("race");
        let started = *since.get_or_insert_with(Instant::now);
        let mut measured = race.storage_speed.lock().expect("race");
        if measured.is_some() || started.elapsed() < PROBE_STORAGE || speed <= 0.0 {
            return;
        }
        *measured = Some(speed);
        drop(measured);
        let storage_secs = self.total.saturating_sub(done) as f64 / speed;
        let faster = race.backup_secs < storage_secs * RACE_MARGIN;
        if faster {
            self.faster.store(true, Ordering::SeqCst);
            self.to_backup.store(true, Ordering::SeqCst);
        }
        ctx.came_from(if faster { "backup" } else { "storage" }, race.speeds(), "");
    }

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
        self.judge(ctx, done, speed);
    }

    fn turning_to_backup(&self) -> bool {
        self.to_backup.load(Ordering::SeqCst)
    }

    fn file_finished(&self, ctx: &Ctx) {
        let n = self.files_done.fetch_add(1, Ordering::SeqCst) + 1;
        ctx.set(|s| s.message = format!("{n} of {} files", self.files));
    }
}

/// Every file of `jobs` into its `.part`, checked, several at once. With
/// `fallback`, gives up as `Stop::Backup` when the storage cannot serve; with
/// a `race`, as `Stop::Faster` when the backup would be faster.
#[allow(clippy::too_many_arguments)]
async fn fetch_parts(
    ctx: &Ctx,
    client: &reqwest::Client,
    source: &Source,
    temp: &Path,
    jobs: &[GameFile],
    have: u64,
    fallback: bool,
    race: Option<&Race>,
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
        fallback: fallback && source.is_storage(),
        to_backup: AtomicBool::new(false),
        race: race.filter(|_| source.is_storage()),
        race_since: Mutex::new(None),
        faster: AtomicBool::new(false),
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
    let mut results = futures_util::stream::iter(tries).buffer_unordered(source.parallel());
    let mut paused = false;
    while let Some(result) = results.next().await {
        match result {
            Ok(()) => {}
            Err(Stop::Paused) => paused = true,
            Err(Stop::Backup) if progress.faster.load(Ordering::SeqCst) => return Err(Stop::Faster),
            // Dropping the others stops them; their parts are kept.
            Err(failed) => return Err(failed),
        }
    }
    drop(results);
    if paused || ctx.stopped() {
        return Err(Stop::Paused);
    }
    ctx.set(|s| s.done = progress.done.load(Ordering::SeqCst));
    Ok(())
}

/// Everything checked: the files go into place, the exe last.
fn place_all(ctx: &Ctx, root: &Path, temp: &Path, jobs: &[GameFile]) -> std::result::Result<(), Stop> {
    ctx.set(|s| {
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
/// the download limit, an expired sign-in), or the storage is switched off.
enum Miss {
    Retry(String),
    Fatal(String),
    Backup,
}

/// One file into its `.part`, checked against its SHA-256. Moving it into
/// place is `place_all`'s, once every file is here.
async fn fetch_one(
    ctx: &Ctx,
    client: &reqwest::Client,
    source: &Source,
    temp: &Path,
    job: &GameFile,
    progress: &Progress<'_>,
) -> std::result::Result<(), Stop> {
    let part = part_path(temp, job);
    let name = job.path.rsplit('/').next().unwrap_or(&job.path).to_string();
    // What the start-up total already counts for this file.
    let mut counted = file_len(&part).min(job.size);
    // The hash of the part's bytes, kept between tries so a retry does not
    // read them again.
    let mut hashed: Option<(u64, Sha256)> = None;
    let mut retries = 0u32;
    // Tries in a row that brought nothing from the storage.
    let mut empty_tries = 0u32;
    loop {
        if ctx.stopped() {
            return Err(Stop::Paused);
        }
        if progress.turning_to_backup() {
            return Err(Stop::Backup);
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
                Ok(url) => {
                    let before = have;
                    let result =
                        stream_into(ctx, client, &url, &name, source.stall(), &part, job.size, &mut have, &mut hasher, progress, &mut counted).await;
                    // Only the storage's own failures count, not the website's.
                    empty_tries = if have > before || ctx.stopped() { 0 } else { empty_tries + 1 };
                    result
                }
                Err(miss) => Err(miss),
            };
            if progress.fallback && (matches!(outcome, Err(Miss::Backup)) || empty_tries >= STORAGE_TRIES) {
                progress.to_backup.store(true, Ordering::SeqCst);
                return Err(Stop::Backup);
            }
            if progress.turning_to_backup() {
                return Err(Stop::Backup);
            }
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
                Err(Miss::Backup) => {
                    return Err(Stop::Failed("Game downloads are not open yet. Try again later -- your progress is kept.".into()))
                }
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
    stall: Duration,
    part: &Path,
    size: u64,
    have: &mut u64,
    hasher: &mut Sha256,
    progress: &Progress<'_>,
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
        if ctx.stopped() || progress.turning_to_backup() {
            break;
        }
        let next = tokio::time::timeout(stall, stream.next())
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
    format!("The download kept failing ({MAX_RETRIES} tries). Your progress is kept -- press Continue to try again later. {STALL_HINT}")
}

/// What a player can do when the connection keeps stopping. 10.10.2026: a
/// player in Russia could not even get the 21 KB file list, three tries of 30 s
/// each ("error decoding response body ... operation timed out"): providers
/// there are known to stall connections to foreign servers after a few KB.
const STALL_HINT: &str = "Your internet connection keeps stopping the download. Some internet providers (in Russia, for example) slow down or block connections to servers abroad, like the ones the game downloads from. Try again with a VPN, or on another network such as a phone hotspot. Once the game is installed, you can play without it.";

/// A list error from the network (not the site refusing it) gets the hint.
fn with_stall_hint(message: String) -> String {
    if message.contains("network: ") && !message.contains("status client error") {
        format!("{message}. {STALL_HINT}")
    } else {
        message
    }
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
        tokio::time::sleep(TICK).await;
    }
    true
}

// ----------------------------------------------------------------- backup ---

/// What the storage did not bring, from the backup archive, into each file's
/// `.part`. Up to `single_files` bytes still missing, one file at a time from
/// inside the archive; more, the whole archive, unpacked here. Every file is
/// checked against the list before it counts.
async fn from_backup(
    ctx: &Ctx,
    client: &reqwest::Client,
    backup: &Backup,
    temp: &Path,
    jobs: &[GameFile],
    single_files: u64,
) -> std::result::Result<(), Stop> {
    std::fs::create_dir_all(temp)?;
    // A part the storage finished was checked as it arrived; a whole one is
    // checked again here all the same (the switch can cut in between).
    let mut left = Vec::new();
    for job in jobs {
        let part = part_path(temp, job);
        let whole = file_len(&part) == job.size && hex(&hash_prefix(&part, job.size).await?.finalize()) == job.sha256;
        if !whole {
            left.push(job.clone());
        }
    }
    if left.is_empty() {
        return Ok(());
    }
    let missing: u64 = left.iter().map(|j| j.size).sum();
    if missing <= single_files {
        // The bar now counts only what is left.
        let source = Source::Archive { url: backup.url.clone(), folder: backup.folder.clone() };
        let have: u64 = left.iter().map(|j| file_len(&part_path(temp, j)).min(j.size)).sum();
        return fetch_parts(ctx, client, &source, temp, &left, have, false, None).await;
    }

    let pieces = Pieces::new(temp, backup.size);
    pieces.clear_others();
    let needed = backup.size - pieces.have() + missing + SPACE_MARGIN;
    if let Some(free) = free_space(temp) {
        if free < needed {
            return Err(Stop::Failed(short_of_space(
                free,
                needed,
                " (the backup copy needs room for its archive and the unpacked files at once)",
                ", then press Continue -- your progress is kept.",
            )));
        }
    }
    download_archive(ctx, client, &mirrors(client, &backup.url).await, &pieces, None).await?;
    let out = temp.join(UNPACK_DIR);
    unpack(ctx, &pieces, &backup.folder, &left, &out).await?;

    // Unpacked: each file checked, then it is that file's part.
    for job in &left {
        let unpacked = target_path(&out.join(backup.folder.trim_end_matches('/')), job);
        let right = file_len(&unpacked) == job.size && hex(&hash_prefix(&unpacked, job.size).await?.finalize()) == job.sha256;
        if !right {
            let _ = std::fs::remove_dir_all(&out);
            return Err(Stop::Failed(format!(
                "{} could not be prepared. Press Continue to try again -- your progress is kept.",
                job.path
            )));
        }
        std::fs::rename(&unpacked, part_path(temp, job))?;
    }
    let _ = std::fs::remove_dir_all(&out);
    pieces.remove();
    Ok(())
}

/// The race's first half (see the top): how long the backup would take here
/// for the `left` bytes still missing, from a few seconds of its archive (the
/// pieces stay for later), plus the unpacking. None when it cannot win: a
/// repair (the storage fetches only those files), no room for the archive, or
/// archive.org not answering.
async fn backup_time(ctx: &Ctx, client: &reqwest::Client, backup: &Backup, temp: &Path, left: u64) -> std::result::Result<Option<Race>, Stop> {
    if left <= BACKUP_SINGLE_FILES {
        return Ok(None);
    }
    std::fs::create_dir_all(temp)?;
    let pieces = Pieces::new(temp, backup.size);
    pieces.clear_others();
    let archive_left = backup.size - pieces.have();
    if free_space(temp).is_some_and(|free| free < archive_left + left + SPACE_MARGIN) {
        return Ok(None);
    }
    let speed = if archive_left == 0 { f64::INFINITY } else { download_archive(ctx, client, &mirrors(client, &backup.url).await, &pieces, Some(PROBE_BACKUP)).await? };
    if speed <= 0.0 {
        return Ok(None);
    }
    Ok(Some(Race {
        backup_secs: archive_left as f64 / speed + left as f64 / UNPACK_SPEED,
        backup_speed: speed,
        storage_speed: Mutex::new(None),
    }))
}

/// The backup archive in the temp folder, as pieces: `backup-<size>.7z.001`,
/// `.002`, ... (a split archive: 7-Zip opens the first and reads on). The size
/// in the name keeps another archive's pieces out.
struct Pieces {
    temp: PathBuf,
    size: u64,
    piece: u64,
}

impl Pieces {
    fn new(temp: &Path, size: u64) -> Self {
        Pieces { temp: temp.to_path_buf(), size, piece: PIECE.max(size.div_ceil(999)) }
    }
    fn count(&self) -> u64 {
        self.size.div_ceil(self.piece)
    }
    fn path(&self, i: u64) -> PathBuf {
        self.temp.join(format!("backup-{}.7z.{:03}", self.size, i + 1))
    }
    fn first(&self) -> PathBuf {
        self.path(0)
    }
    /// Where piece `i` starts in the archive, and its length.
    fn span(&self, i: u64) -> (u64, u64) {
        let start = i * self.piece;
        (start, self.piece.min(self.size - start))
    }
    fn have(&self) -> u64 {
        (0..self.count()).map(|i| file_len(&self.path(i)).min(self.span(i).1)).sum()
    }
    /// Other archives' pieces, and the previous launcher's one-file archive.
    fn clear_others(&self) {
        let ours = format!("backup-{}.7z.", self.size);
        let _ = std::fs::remove_file(self.temp.join(WHOLE_ARCHIVE));
        for entry in std::fs::read_dir(&self.temp).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_piece(&name) && !name.starts_with(&ours) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    fn remove(&self) {
        for i in 0..self.count() {
            let _ = std::fs::remove_file(self.path(i));
        }
    }
}

/// `backup-<size>.7z.<3 digits>`: a piece of the backup archive.
fn is_piece(name: &str) -> bool {
    name.strip_prefix("backup-")
        .and_then(|rest| rest.split_once(".7z."))
        .is_some_and(|(size, n)| !size.is_empty() && size.bytes().all(|b| b.is_ascii_digit()) && n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The archive's address on each archive.org server that holds it (the
/// item's metadata, `workable_servers`): archive.org's own link sends
/// everyone to the same one. Just `url` when that cannot be found (or it is
/// not an archive.org link).
async fn mirrors(client: &reqwest::Client, url: &str) -> Vec<String> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Meta {
        dir: String,
        workable_servers: Vec<String>,
    }
    let mut found = Vec::new();
    if let Some((item, file)) = url.strip_prefix("https://archive.org/download/").and_then(|rest| rest.split_once('/')) {
        let meta: Option<Meta> = async {
            let resp = client.get(format!("https://archive.org/metadata/{item}")).timeout(Duration::from_secs(15)).send().await.ok()?;
            resp.json().await.ok()
        }
        .await;
        let plain = |text: &str, extra: &[u8]| !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b));
        if let Some(meta) = meta.filter(|m| m.dir.starts_with('/') && !m.dir.contains("..") && plain(&m.dir, b"/._-")) {
            for server in meta.workable_servers {
                // Only archive.org's own servers.
                if server.ends_with(".archive.org") && plain(&server, b".-") {
                    found.push(format!("https://{server}{}/{file}", meta.dir));
                }
            }
        }
    }
    if found.is_empty() {
        found.push(url.to_string());
    }
    found
}

/// The archive's download, shared by the pieces downloading at once.
struct Pull<'a> {
    ctx: &'a Ctx,
    client: &'a reqwest::Client,
    servers: Vec<String>,
    pieces: &'a Pieces,
    /// Pieces not finished yet; a stopped one goes back to the front.
    todo: Mutex<std::collections::VecDeque<u64>>,
    done: AtomicU64,
    meter: Mutex<Meter>,
    last_emit: Mutex<Instant>,
    /// The race's few seconds: until then, and the bar is left alone.
    until: Option<Instant>,
    failed: Mutex<Option<String>>,
}

impl Pull<'_> {
    fn over(&self) -> bool {
        self.ctx.stopped() || self.until.is_some_and(|t| Instant::now() >= t) || self.failed.lock().expect("pull").is_some()
    }

    fn tick(&self) {
        {
            let mut last = self.last_emit.lock().expect("last emit");
            if last.elapsed() < EMIT_EVERY {
                return;
            }
            *last = Instant::now();
        }
        let done = self.done.load(Ordering::SeqCst);
        let speed = self.meter.lock().expect("meter").rate();
        let probing = self.until.is_some();
        self.ctx.set(|s| {
            s.speed = speed;
            if !probing {
                s.done = done;
                s.eta_secs = eta(self.pieces.size.saturating_sub(done), speed);
                s.message.clear();
            }
        });
        self.ctx.emit();
    }

    /// One connection: piece after piece, starting on server `first`; a
    /// failure moves it to the next server.
    async fn worker(&self, first: usize) {
        let mut server = first % self.servers.len();
        loop {
            if self.over() {
                return;
            }
            let Some(i) = self.todo.lock().expect("pull").pop_front() else { return };
            let mut retries = 0u32;
            loop {
                match self.piece(&self.servers[server], i).await {
                    Ok(true) => break,
                    Ok(false) => {
                        self.todo.lock().expect("pull").push_front(i);
                        return;
                    }
                    Err(why) => {
                        server = (server + 1) % self.servers.len();
                        // The race's few seconds are not spent waiting.
                        if self.until.is_some() || self.over() {
                            self.todo.lock().expect("pull").push_front(i);
                            return;
                        }
                        if !backoff(self.ctx, &mut retries, why).await {
                            *self.failed.lock().expect("pull") = Some(retry_exhausted());
                            return;
                        }
                    }
                }
            }
        }
    }

    /// The rest of piece `i` from `url`, appended to its file: true once it is
    /// whole, false if stopped first.
    async fn piece(&self, url: &str, i: u64) -> std::result::Result<bool, String> {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;

        let path = self.pieces.path(i);
        let (start, len) = self.pieces.span(i);
        let mut have = file_len(&path);
        if have > len {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
            have = 0;
        }
        if have == len {
            return Ok(true);
        }
        let from = start + have;
        let resp = self
            .client
            .get(url)
            .header(reqwest::header::RANGE, format!("bytes={from}-{}", start + len - 1))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("the server answered {}", resp.status()));
        }
        // Exactly the bytes asked for, or none of them.
        let range = resp.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).unwrap_or("");
        if !range.starts_with(&format!("bytes {from}-")) {
            return Err("the server sent another part of the file".into());
        }
        let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await.map_err(|e| e.to_string())?;
        let mut stream = resp.bytes_stream();
        let mut last_flush = Instant::now();
        while !self.over() {
            let next = tokio::time::timeout(STALL_TIMEOUT, stream.next()).await.map_err(|_| "the connection stalled".to_string())?;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| e.to_string())?;
            if have + chunk.len() as u64 > len {
                return Err("the server sent more than was asked".into());
            }
            file.write_all(&chunk).await.map_err(|e| format!("writing to disk failed: {e}"))?;
            have += chunk.len() as u64;
            self.done.fetch_add(chunk.len() as u64, Ordering::SeqCst);
            self.meter.lock().expect("meter").add(chunk.len() as u64);
            self.tick();
            if last_flush.elapsed() > Duration::from_secs(5) {
                file.flush().await.map_err(|e| e.to_string())?;
                last_flush = Instant::now();
            }
        }
        file.flush().await.map_err(|e| e.to_string())?;
        if have == len || self.over() {
            Ok(have == len)
        } else {
            Err("the connection closed early".into())
        }
    }
}

/// The backup archive into its pieces, ARCHIVE_PARALLEL at a time over
/// `servers` (its addresses, see `mirrors`); each piece resumes where it
/// stopped. With `probe`, only for that long (the race): the speed it
/// reached, in bytes per second.
async fn download_archive(
    ctx: &Ctx,
    client: &reqwest::Client,
    servers: &[String],
    pieces: &Pieces,
    probe: Option<Duration>,
) -> std::result::Result<f64, Stop> {
    let servers = servers.to_vec();
    std::fs::create_dir_all(&pieces.temp)?;
    let have = pieces.have();
    let todo: std::collections::VecDeque<u64> = (0..pieces.count()).filter(|&i| file_len(&pieces.path(i)) != pieces.span(i).1).collect();
    if todo.is_empty() {
        return Ok(f64::INFINITY);
    }
    ctx.set(|s| {
        if probe.is_some() {
            s.message = "Finding the fastest download server...".into();
        } else {
            s.phase = Phase::Downloading;
            s.done = have;
            s.total = pieces.size;
            s.message = if have > 0 { "Resuming...".into() } else { "Downloading...".into() };
        }
    });
    ctx.emit();
    let pull = Pull {
        ctx,
        client,
        servers,
        pieces,
        todo: Mutex::new(todo),
        done: AtomicU64::new(have),
        meter: Mutex::new(Meter::new()),
        last_emit: Mutex::new(Instant::now()),
        until: probe.map(|d| Instant::now() + d),
        failed: Mutex::new(None),
    };
    futures_util::future::join_all((0..ARCHIVE_PARALLEL).map(|w| pull.worker(w))).await;
    let speed = pull.meter.lock().expect("meter").rate();
    if probe.is_some() {
        ctx.set(|s| s.speed = 0.0);
        return if ctx.stopped() { Err(Stop::Paused) } else { Ok(speed) };
    }
    if let Some(why) = pull.failed.lock().expect("pull").take() {
        return Err(Stop::Failed(why));
    }
    if ctx.stopped() {
        return Err(Stop::Paused);
    }
    if pieces.have() != pieces.size {
        return Err(Stop::Failed("The download stopped before the end. Press Continue -- your progress is kept.".into()));
    }
    Ok(speed)
}

/// 7-Zip's standalone extractor, compiled into the launcher (build.rs,
/// resources/README.md); it only runs for the backup.
#[cfg(has_7zr)]
const SEVEN_ZIP: &[u8] = include_bytes!("../resources/7zr.exe");
#[cfg(not(has_7zr))]
const SEVEN_ZIP: &[u8] = &[];

/// The 7-Zip to unpack with: the one inside the launcher (written once to the
/// config dir), else an installed 7-Zip.
fn seven_zip(config_dir: &Path) -> Option<PathBuf> {
    if !SEVEN_ZIP.is_empty() {
        let path = config_dir.join("tools").join("7zr.exe");
        if std::fs::read(&path).is_ok_and(|b| b == SEVEN_ZIP) {
            return Some(path);
        }
        if std::fs::create_dir_all(path.parent()?).is_ok() && std::fs::write(&path, SEVEN_ZIP).is_ok() {
            return Some(path);
        }
    }
    for base in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(dir) = std::env::var(base) {
            let exe = Path::new(&dir).join("7-Zip").join("7z.exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn command(exe: &Path) -> std::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Only `jobs`' files out of the archive, into `out` (under `folder`, as in
/// the archive). The list of names goes through a file: hundreds of paths do
/// not fit on a command line.
async fn unpack(ctx: &Ctx, pieces: &Pieces, folder: &str, jobs: &[GameFile], out: &Path) -> std::result::Result<(), Stop> {
    let archive = pieces.first();
    let exe = seven_zip(&ctx.config_dir).ok_or_else(|| {
        Stop::Failed("The game's files could not be prepared: 7-Zip is missing. Install 7-Zip from 7-zip.org and press Continue -- your progress is kept.".into())
    })?;
    let total: u64 = jobs.iter().map(|j| j.size).sum();
    ctx.set(|s| {
        s.phase = Phase::Preparing;
        s.done = 0;
        s.total = total;
        s.speed = 0.0;
        s.eta_secs = None;
        s.message = "Preparing files...".into();
    });
    ctx.emit();

    let list = archive.with_file_name("unpack.txt");
    let names: String = jobs.iter().map(|j| format!("{folder}{}\n", j.path)).collect();
    std::fs::write(&list, names)?;
    let _ = std::fs::remove_dir_all(out);

    let status = ctx.status.clone();
    let stop = ctx.stop.clone();
    let tell = ctx.tell.clone();
    let dir = ctx.dir.clone();
    let (out_dir, list_file) = (out.to_path_buf(), list.clone());
    let result = tokio::task::spawn_blocking(move || -> std::result::Result<bool, String> {
        use std::io::Read as _;
        // -bsp1: progress to stdout; -bso0: no file list; -aoa: overwrite, so
        // a second try after a pause just redoes it; -scsUTF-8: the list's
        // names are UTF-8.
        let mut child = command(&exe)
            .arg("x")
            .arg(&archive)
            .arg(format!("-o{}", out_dir.display()))
            .args(["-y", "-aoa", "-bso0", "-bsp1", "-bse2", "-scsUTF-8"])
            .arg(format!("@{}", list_file.display()))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start 7-Zip: {e}"))?;
        let mut stdout = child.stdout.take().ok_or("7-Zip gave no output")?;
        let mut buf = [0u8; 4096];
        let mut pending = String::new();
        let mut last = Instant::now();
        loop {
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
            let n = stdout.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
            // 7-Zip redraws its progress line with backspaces and CRs: "42% 17".
            let pct = pending
                .split(['\r', '\n', '\u{8}'])
                .filter_map(|piece| piece.trim().split('%').next().filter(|_| piece.contains('%')).and_then(|p| p.trim().parse::<u64>().ok()))
                .rfind(|p| *p <= 100);
            if let Some(cut) = pending.rfind(['\r', '\n', '\u{8}']) {
                pending = pending[cut + 1..].to_string();
            }
            if let (Some(pct), true) = (pct, last.elapsed() >= EMIT_EVERY) {
                last = Instant::now();
                let snapshot = {
                    let mut st = status.lock().expect("download status");
                    st.done = st.total * pct / 100;
                    st.message = format!("Preparing files... {pct}%");
                    st.clone()
                };
                tell(Event::Status(Status { free_bytes: free_space(&dir), ..snapshot }));
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
    .map_err(|e| Stop::Failed(e.to_string()))?;
    let _ = std::fs::remove_file(&list);
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(Stop::Paused),
        Err(why) => {
            // A damaged archive is downloaded again next time.
            pieces.remove();
            Err(Stop::Failed(format!("The game's files could not be prepared ({why}). Press Continue to try again.")))
        }
    }
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

/// The bytes of the downloads waiting in `temp` (`<sha256>.part` files and
/// the backup archive's pieces), or None when there are none.
fn parts_on_disk(temp: &Path) -> Option<u64> {
    let mut found = None;
    for entry in std::fs::read_dir(temp).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_part = name
            .strip_suffix(".part")
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()));
        if is_part || is_piece(&name) {
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

/// Not enough room: what is free rounded down, what is needed rounded up, and how much more to
/// free, so the two never read the same ("58.2 GB free, about 58.2 GB needed" was a refusal).
fn short_of_space(free: u64, needed: u64, why: &str, then: &str) -> String {
    let tenths = |n: u64| n as f64 / (1024.0 * 1024.0 * 1024.0) * 10.0;
    let down = |n: u64| tenths(n).floor() / 10.0;
    let up = |n: u64| (tenths(n).ceil() / 10.0).max(0.1);
    format!(
        "Not enough space on that drive: {:.1} GB free, {:.1} GB needed{why}. Free at least {:.1} GB more{then}",
        down(free),
        up(needed).max(down(free) + 0.1),
        up(needed.saturating_sub(free)),
    )
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

    /// Verify files leaves a recognised DLSS / XeSS swap alone and repairs anything else there.
    #[test]
    fn verify_keeps_only_recognised_upscaler_swaps() {
        let root = tempfile::tempdir().unwrap();
        let xess = "Engine/Plugins/Runtime/Intel/XeSS/Binaries/ThirdParty/Win64/libxess.dll";
        let file = target_path(root.path(), &GameFile { path: xess.into(), size: 0, sha256: String::new() });
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let list = vec![GameFile { path: xess.into(), size: 15, sha256: "e".repeat(64) }];
        std::fs::write(&file, b"not a xess build, longer").unwrap();
        assert!(kept_swaps(root.path(), &list).is_empty());
        let Some(game_dir) = std::env::var_os("SP_TEST_GAME_DIR") else { return };
        let real = xess.split('/').fold(std::path::PathBuf::from(game_dir), |d, part| d.join(part));
        std::fs::copy(real, &file).unwrap();
        assert_eq!(kept_swaps(root.path(), &list), vec![(xess.to_string(), "XeSS 1.0.1.12 (libxess.dll)".to_string())]);
    }

    fn sha(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    fn entry(path: &str, body: &[u8]) -> GameFile {
        GameFile { path: path.into(), size: body.len() as u64, sha256: sha(body) }
    }

    /// Bytes that do not compress (xorshift), like the game's packed files.
    fn noise(len: usize) -> Vec<u8> {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect()
    }

    fn list(files: Vec<GameFile>) -> GameList {
        GameList { files, backup: None }
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

    // ---- the list (fetch_list_from) ---------------------------------------

    /// A valid list, as the site sends it.
    fn list_json() -> Vec<u8> {
        serde_json::json!({ "files": [{ "path": crate::game::GAME_EXE, "size": 3, "sha256": sha(b"abc") }] })
            .to_string()
            .into_bytes()
    }

    /// Answers each request with `answers[n]` (the last one again after that):
    /// `Some(status, body, cut)` sends the head and the body, only its first
    /// half when `cut`; `None` closes the connection at once. Keeps every request.
    async fn serve_list(answers: Vec<Option<(u16, Vec<u8>, bool)>>) -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let index = {
                    let mut seen = kept.lock().unwrap();
                    seen.push(String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase());
                    seen.len() - 1
                };
                let Some((status, body, cut)) = answers[index.min(answers.len() - 1)].clone() else { continue };
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(if cut { &body[..body.len() / 2] } else { &body }).await;
            }
        });
        (format!("http://{addr}/api/launcher/game"), seen)
    }

    const NO_WAIT: [Duration; 2] = [Duration::from_millis(1), Duration::from_millis(1)];

    #[tokio::test]
    async fn the_list_is_asked_again_when_its_answer_is_cut_off() {
        let (url, seen) = serve_list(vec![Some((200, list_json(), true)), None, Some((200, list_json(), false))]).await;
        let list = fetch_list_from(&url, &NO_WAIT).await.expect("the third try brings the list");
        assert_eq!(list.files.len(), 1);
        assert_eq!(seen.lock().unwrap().len(), 3, "a cut-off answer and a dropped connection were asked again");
    }

    #[test]
    fn a_stalled_connection_says_what_the_player_can_do() {
        let stalled = with_stall_hint("Could not get the list: network: error decoding response body: operation timed out (3 tries)".into());
        assert!(stalled.ends_with(STALL_HINT) && stalled.contains("VPN"), "{stalled}");
        let refused = with_stall_hint("Could not get the list: network: HTTP status client error (403 Forbidden)".into());
        assert!(!refused.contains("VPN"), "the site refusing is no connection problem: {refused}");
        let invalid = with_stall_hint("Could not get the list: the list is not valid (no files)".into());
        assert!(!invalid.contains("VPN"));
        assert!(retry_exhausted().contains("VPN"), "a download that kept failing says it too");
    }

    #[tokio::test]
    async fn a_list_that_never_arrives_says_why_and_how_often() {
        let (url, seen) = serve_list(vec![Some((200, list_json(), true))]).await;
        let e = fetch_list_from(&url, &NO_WAIT).await.expect_err("never whole").to_string();
        assert_eq!(seen.lock().unwrap().len(), 3, "three tries in all");
        assert!(e.starts_with("network: error decoding response body: "), "the cause follows: {e}");
        assert!(e.ends_with(" (3 tries)"), "{e}");
    }

    #[tokio::test]
    async fn a_refused_list_is_not_asked_again() {
        let (url, seen) = serve_list(vec![Some((404, b"{}".to_vec(), false))]).await;
        let e = fetch_list_from(&url, &NO_WAIT).await.expect_err("404").to_string();
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert!(e.contains("404") && !e.contains("tries"), "{e}");
    }

    #[tokio::test]
    async fn the_list_is_asked_compressed_and_the_game_files_never_are() {
        let (url, seen) = serve_list(vec![Some((200, list_json(), false))]).await;
        fetch_list_from(&url, &NO_WAIT).await.expect("the list");
        let _ = http_client().unwrap().get(&url).send().await;
        let seen = seen.lock().unwrap();
        assert!(seen[0].contains("accept-encoding: gzip"), "the list: {}", seen[0]);
        assert!(!seen[1].contains("gzip"), "a game file: {}", seen[1]);
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

    /// The storage's whole run, as `run` does it: the parts, then into place.
    async fn fetch_all(ctx: &Ctx, client: &reqwest::Client, source: &Source, root: &Path, temp: &Path, jobs: &[GameFile], have: u64) -> std::result::Result<(), Stop> {
        fetch_parts(ctx, client, source, temp, jobs, have, false, None).await?;
        place_all(ctx, root, temp, jobs)
    }

    /// A server for the backup tests: each path its body, with or without
    /// `Range` (archive.org answers a file inside an archive whole, and the
    /// archive itself in ranges, `bytes=a-` or `bytes=a-b`); any other path
    /// answers `status`.
    async fn serve_paths(routes: Vec<(String, Vec<u8>, bool)>, status: u16) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let Some((_, body, ranges)) = routes.iter().find(|(p, _, _)| *p == path) else {
                        let head = format!("HTTP/1.1 {status} Nope\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        let _ = sock.write_all(head.as_bytes()).await;
                        return;
                    };
                    let range: Option<(usize, usize)> = req
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string))
                        .filter(|_| *ranges)
                        .and_then(|r| {
                            let (a, b) = r.trim().split_once('-')?;
                            Some((a.parse().ok()?, b.parse().unwrap_or(body.len() - 1).min(body.len() - 1)))
                        });
                    let (from, to) = range.unwrap_or((0, body.len() - 1));
                    let head = if range.is_some() {
                        format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{to}/{}\r\nConnection: close\r\n\r\n", to + 1 - from, body.len())
                    } else {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    };
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body[from..=to]).await;
                });
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_storage_that_cannot_serve_turns_to_the_backup() {
        let base = serve_paths(vec![], 503).await;
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let client = http_client().unwrap();
        let jobs = vec![entry("BravoHotelGame/a.pak", b"aaaa"), entry("BravoHotelGame/b.pak", b"bbbb")];
        let source = Source::Base(format!("{base}/game/"));
        let result = fetch_parts(&ctx, &client, &source, &tmp.path().join(TEMP_DIR), &jobs, 0, true, None).await;
        assert!(matches!(result, Err(Stop::Backup)));
    }

    /// Serves `body` at any path with `Range`, `per_ms` bytes a millisecond:
    /// a slow storage.
    async fn serve_slowly(body: Vec<u8>, per_ms: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let from: usize = req
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(|r| r.trim_end_matches('-').to_string()))
                        .and_then(|r| r.parse().ok())
                        .unwrap_or(0);
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {from}-{}/{}\r\nConnection: close\r\n\r\n",
                        body.len() - from,
                        body.len() - 1,
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    for chunk in body[from..].chunks(per_ms) {
                        if sock.write_all(chunk).await.is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                });
            }
        });
        format!("http://{addr}/game/")
    }

    fn race(backup_secs: f64) -> Race {
        Race { backup_secs, backup_speed: 1e9, storage_speed: Mutex::new(None) }
    }

    #[tokio::test]
    async fn the_race_hands_over_to_a_faster_backup() {
        let body: Vec<u8> = (0..2_000_000u32).map(|i| (i % 241) as u8).collect();
        let base = serve_slowly(body.clone(), 2_000).await;
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let client = http_client().unwrap();
        let jobs = vec![entry("BravoHotelGame/big.pak", &body)];
        let temp = tmp.path().join(TEMP_DIR);
        let fast = race(0.0);
        let result = fetch_parts(&ctx, &client, &Source::Base(base.clone()), &temp, &jobs, 0, true, Some(&fast)).await;
        assert!(matches!(result, Err(Stop::Faster)), "the backup would finish at once");
        assert!(fast.storage_speed.lock().unwrap().is_some_and(|s| s > 0.0), "the storage's speed was measured");
        assert!(file_len(&part_path(&temp, &jobs[0])) > 0, "what the storage sent is kept");

        // A backup slower than the storage: the storage finishes.
        let slow = race(1e9);
        let result = fetch_parts(&ctx, &client, &Source::Base(base), &temp, &jobs, 0, true, Some(&slow)).await;
        assert!(result.is_ok());
        assert!(slow.storage_speed.lock().unwrap().is_some());
    }

    /// A ctx that keeps its #launcher-logs reports.
    fn reporting_ctx(dir: &Path) -> (Ctx, Arc<Mutex<Vec<serde_json::Value>>>) {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let kept = reports.clone();
        let ctx = Ctx {
            tell: Arc::new(move |event| {
                if let Event::Report(report) = event {
                    kept.lock().unwrap().push(report);
                }
            }),
            ..test_ctx(dir)
        };
        (ctx, reports)
    }

    #[tokio::test]
    async fn the_start_is_told_once_the_race_picks_a_source() {
        let body: Vec<u8> = (0..2_000_000u32).map(|i| (i % 241) as u8).collect();
        let base = serve_slowly(body.clone(), 2_000).await;
        let client = http_client().unwrap();
        let jobs = vec![entry("BravoHotelGame/big.pak", &body)];
        for (backup_secs, winner) in [(0.0, "backup"), (1e9, "storage")] {
            let tmp = tempfile::tempdir().unwrap();
            let (ctx, reports) = reporting_ctx(tmp.path());
            ctx.start.lock().unwrap().due = Some((1, body.len() as u64));
            let judged = race(backup_secs);
            let _ = fetch_parts(&ctx, &client, &Source::Base(base.clone()), &tmp.path().join(TEMP_DIR), &jobs, 0, true, Some(&judged)).await;
            let reports = reports.lock().unwrap();
            assert_eq!(reports.len(), 1, "told once: {reports:?}");
            assert_eq!(reports[0]["action"], "download.started");
            assert_eq!(reports[0]["source"], winner);
            assert!(reports[0]["speeds"]["storage"].as_f64().is_some_and(|s| s > 0.0), "with both speeds");
        }
    }

    #[test]
    fn a_new_download_tells_its_source_then_each_switch() {
        let tmp = tempfile::tempdir().unwrap();
        let (ctx, reports) = reporting_ctx(tmp.path());
        // A verify or a resume: nothing.
        ctx.came_from("storage", None, "");
        assert!(reports.lock().unwrap().is_empty());

        ctx.start.lock().unwrap().due = Some((450, 31_000_000_000));
        ctx.came_from("storage", None, "");
        ctx.came_from("storage", None, "");
        ctx.came_from("backup", None, "The storage could not send the files.");
        let reports = reports.lock().unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0]["action"], "download.started");
        assert_eq!(reports[0]["source"], "storage");
        assert_eq!(reports[0]["files"], 450);
        assert_eq!(reports[1]["action"], "download.switched");
        assert_eq!(reports[1]["source"], "backup");
        assert_eq!(reports[1]["reason"], "The storage could not send the files.");
    }

    /// The real backup on archive.org, for a few seconds: both servers found,
    /// the speed it reaches here. `cargo test -- --ignored real_backup`.
    #[tokio::test]
    #[ignore = "downloads from archive.org"]
    async fn real_backup_servers_and_speed() {
        let url = "https://archive.org/download/SPShippingDev/Manifest%20%232065353802481281242.7z";
        let client = http_client().unwrap();
        let servers = mirrors(&client, url).await;
        println!("servers: {servers:?}");
        assert!(servers.len() >= 2 && servers.iter().all(|s| s.contains(".us.archive.org/")), "{servers:?}");
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let pieces = Pieces::new(tmp.path(), 29_710_037_796);
        let speed = download_archive(&ctx, &client, &servers, &pieces, Some(Duration::from_secs(8))).await.ok().unwrap();
        println!("{:.1} MB/s, {} MB kept in {} pieces", speed / 1e6, pieces.have() / 1_000_000, (0..pieces.count()).filter(|&i| pieces.path(i).exists()).count());
        assert!(speed > 0.0);
        let first = std::fs::read(pieces.first()).unwrap();
        assert!(first.starts_with(b"7z\xbc\xaf\x27\x1c"), "the first piece is the archive's start");
    }

    #[test]
    fn an_update_pauses_the_download_and_the_next_start_continues_it_once() {
        let tmp = tempfile::tempdir().unwrap();
        let config = tmp.path().join("config");
        std::fs::create_dir_all(&config).unwrap();
        let dl = Downloader::new(config.clone());
        assert!(!pause_for_update(&dl), "nothing running: nothing to continue");

        // A Verify files run in progress, which stops when asked, as the worker does.
        store_saved(&config, &Saved { dir: "D:\\Games\\SP".into(), verify: true, ..Default::default() }).unwrap();
        dl.running.store(true, Ordering::SeqCst);
        let (stop, running) = (dl.stop.clone(), dl.running.clone());
        let worker = std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            running.store(false, Ordering::SeqCst);
        });
        assert!(pause_for_update(&dl));
        worker.join().unwrap();
        assert!(!dl.is_running(), "stopped before the update installs");

        assert_eq!(take_resume(&dl), Some(Resume { dir: "D:\\Games\\SP".into(), verify: true }));
        assert_eq!(take_resume(&dl), None, "it continues once");
        assert_eq!(load_saved(&config).dir, "D:\\Games\\SP", "the run itself is kept");
    }

    #[tokio::test]
    async fn verify_files_continues_where_it_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        std::fs::create_dir_all(&ctx.config_dir).unwrap();
        let (a, b) = (b"aaaa-original".to_vec(), b"bbbb-original".to_vec());
        let files = vec![entry("BravoHotelGame/a.pak", &a), entry("BravoHotelGame/b.pak", &b)];
        let root = tmp.path();
        std::fs::create_dir_all(root.join("BravoHotelGame")).unwrap();
        std::fs::write(root.join("BravoHotelGame/a.pak"), &a).unwrap();
        std::fs::write(root.join("BravoHotelGame/b.pak"), &b).unwrap();

        // A first run finds both fine and remembers them.
        let mut saved = Saved { dir: root.to_string_lossy().into_owned(), verify: true, ..Default::default() };
        assert!(damaged(&ctx, root, &files, &[], &mut saved).await.ok().unwrap().is_empty());
        assert_eq!(saved.fine.len(), 2);
        assert_eq!(load_saved(&ctx.config_dir).fine.len(), 2, "kept on disk for a restart");

        // a changes but keeps its size and time: a continued run does not read it
        // again (the proof it was skipped). b changes and its time moves on.
        let a_path = root.join("BravoHotelGame/a.pak");
        let a_time = std::fs::metadata(&a_path).unwrap().modified().unwrap();
        std::fs::write(&a_path, b"aaaa-changed!").unwrap();
        std::fs::File::options().write(true).open(&a_path).unwrap().set_modified(a_time).unwrap();
        let b_path = root.join("BravoHotelGame/b.pak");
        std::fs::write(&b_path, b"bbbb-changed!").unwrap();
        let later = std::fs::metadata(&b_path).unwrap().modified().unwrap() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&b_path).unwrap().set_modified(later).unwrap();

        let bad = damaged(&ctx, root, &files, &[], &mut saved).await.ok().unwrap();
        assert_eq!(bad.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["BravoHotelGame/b.pak"]);

        // A new expected checksum is checked again whatever the file's time.
        let renewed = vec![entry("BravoHotelGame/a.pak", b"aaaa-newbuild"), files[1].clone()];
        let bad = damaged(&ctx, root, &renewed, &[], &mut saved).await.ok().unwrap();
        assert!(bad.iter().any(|f| f.path == "BravoHotelGame/a.pak"));
    }

    #[test]
    fn pieces_are_named_and_cut_for_seven_zip() {
        let temp = Path::new("t");
        let pieces = Pieces::new(temp, 3 * PIECE + 10);
        assert_eq!(pieces.count(), 4);
        assert_eq!(pieces.span(3), (3 * PIECE, 10));
        assert_eq!(pieces.path(0), temp.join(format!("backup-{}.7z.001", 3 * PIECE + 10)));
        // A huge archive still fits in three-digit names.
        assert!(Pieces::new(temp, 2000 * PIECE).count() <= 999);
        assert!(is_piece("backup-29710037796.7z.042"));
        assert!(!is_piece("backup-.7z.001") && !is_piece("backup-12.7z.1") && !is_piece("game.7z.part"));
    }

    #[tokio::test]
    async fn the_archive_comes_in_pieces_from_the_servers_that_work() {
        let archive = noise(5 * PIECE as usize + 777);
        let good = serve_paths(vec![("/item/game.7z".into(), archive.clone(), true)], 404).await;
        let broken = serve_paths(vec![], 503).await;
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let temp = tmp.path().join(TEMP_DIR);
        std::fs::create_dir_all(&temp).unwrap();
        let pieces = Pieces::new(&temp, archive.len() as u64);
        // An earlier run: the first piece whole, half of the third.
        std::fs::write(pieces.path(0), &archive[..PIECE as usize]).unwrap();
        std::fs::write(pieces.path(2), &archive[2 * PIECE as usize..2 * PIECE as usize + 1000]).unwrap();
        let servers = vec![format!("{broken}/item/game.7z"), format!("{good}/item/game.7z")];
        let client = http_client().unwrap();
        let result = download_archive(&ctx, &client, &servers, &pieces, None).await;
        assert!(result.is_ok(), "{:?}", result.err().map(|e| match e { Stop::Failed(m) => m, _ => "stopped".into() }));
        let joined: Vec<u8> = (0..pieces.count()).flat_map(|i| std::fs::read(pieces.path(i)).unwrap()).collect();
        assert!(joined == archive, "the pieces put together are the archive");
    }

    #[tokio::test]
    async fn the_backup_sends_single_files_whole() {
        let folder = "Manifest #2065353802481281242/";
        let bodies = vec![
            (crate::game::GAME_EXE.to_string(), b"MZ the exe".to_vec()),
            ("BravoHotelGame/Content/Paks/a b.pak".to_string(), (0..50_000u32).map(|i| (i % 13) as u8).collect::<Vec<u8>>()),
        ];
        // Like archive.org: the whole inner path encoded, no ranges.
        let routes = bodies.iter().map(|(p, b)| (format!("/item/game.7z/{}", encode_all(&format!("{folder}{p}"))), b.clone(), false)).collect();
        let base = serve_paths(routes, 404).await;
        let files: Vec<GameFile> = bodies.iter().map(|(p, b)| entry(p, b)).collect();
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let temp = tmp.path().join(TEMP_DIR);
        // Half of one file came from the storage before it stopped serving.
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(part_path(&temp, &files[1]), &bodies[1].1[..1000]).unwrap();
        let backup = Backup { url: format!("{base}/item/game.7z"), folder: folder.into(), size: 0, only: false };
        let client = http_client().unwrap();
        let result = from_backup(&ctx, &client, &backup, &temp, &files, u64::MAX).await;
        assert!(result.is_ok(), "{:?}", result.err().map(|e| match e { Stop::Failed(m) => m, _ => "stopped".into() }));
        place_all(&ctx, tmp.path(), &temp, &files).ok().unwrap();
        for (path, body) in &bodies {
            assert_eq!(&std::fs::read(tmp.path().join(path)).unwrap(), body, "{path}");
        }
    }

    #[tokio::test]
    async fn the_backup_archive_is_downloaded_unpacked_and_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = test_ctx(tmp.path());
        let Some(seven) = seven_zip(&ctx.config_dir) else {
            eprintln!("skipped: no 7-Zip here (resources/7zr.exe or an installed 7-Zip)");
            return;
        };
        // An archive laid out like the real one: the game under one folder,
        // plus a file the list does not have.
        let folder = "Manifest #2065353802481281242";
        let stage = tmp.path().join("stage");
        let bodies = vec![
            (crate::game::GAME_EXE.to_string(), b"MZ the exe".to_vec()),
            // Noise, so the archive is several pieces long.
            (
                "BravoHotelGame/Content/Paks/pakchunk0-WindowsClient.pak".to_string(),
                noise(300_000),
            ),
            ("Engine/Binaries/x.dll".to_string(), b"dll".to_vec()),
        ];
        for (path, body) in &bodies {
            let file = stage.join(folder).join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, body).unwrap();
        }
        std::fs::create_dir_all(stage.join(folder).join("BravoHotelGame/Binaries/Win64/BattlEye")).unwrap();
        std::fs::write(stage.join(folder).join("BravoHotelGame/Binaries/Win64/BattlEye/BEService_x64.exe"), b"not ours").unwrap();
        let archive = tmp.path().join("game.7z");
        let made = command(&seven).current_dir(&stage).arg("a").arg(&archive).arg(folder).output().unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stdout));
        let archive_bytes = std::fs::read(&archive).unwrap();

        let base = serve_paths(vec![("/item/game.7z".into(), archive_bytes.clone(), true)], 404).await;
        let files: Vec<GameFile> = bodies.iter().map(|(p, b)| entry(p, b)).collect();
        let temp = tmp.path().join("game").join(TEMP_DIR);
        // One file came whole from the storage already: only the others are unpacked.
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(part_path(&temp, &files[2]), &bodies[2].1).unwrap();
        // An earlier run: the first piece, and the previous launcher's
        // one-file archive (which goes).
        let pieces = Pieces::new(&temp, archive_bytes.len() as u64);
        assert!(pieces.count() >= 3, "{} pieces", pieces.count());
        std::fs::write(pieces.path(0), &archive_bytes[..PIECE as usize]).unwrap();
        std::fs::write(temp.join(WHOLE_ARCHIVE), &archive_bytes[..100]).unwrap();
        let backup = Backup { url: format!("{base}/item/game.7z"), folder: format!("{folder}/"), size: archive_bytes.len() as u64, only: false };
        let client = http_client().unwrap();
        let result = from_backup(&ctx, &client, &backup, &temp, &files, 0).await;
        assert!(result.is_ok(), "{:?}", result.err().map(|e| match e { Stop::Failed(m) => m, _ => "stopped".into() }));
        let root = tmp.path().join("game");
        place_all(&ctx, &root, &temp, &files).ok().unwrap();
        for (path, body) in &bodies {
            assert_eq!(&std::fs::read(root.join(path)).unwrap(), body, "{path}");
        }
        assert!(!pieces.first().exists(), "the archive is deleted once unpacked");
        assert!(!temp.join(WHOLE_ARCHIVE).exists(), "the old one-file archive is gone");
        assert!(!root.join("BravoHotelGame/Binaries/Win64/BattlEye").exists(), "only the list's files are unpacked");
    }

    fn test_ctx(dir: &Path) -> Ctx {
        Ctx {
            tell: Arc::new(|_| {}),
            status: Arc::new(Mutex::new(Status::idle(dir.to_string_lossy().into_owned()))),
            stop: Arc::new(AtomicBool::new(false)),
            config_dir: dir.join("config"),
            dir: dir.to_path_buf(),
            start: Mutex::default(),
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
            Stop::Backup => "backup".into(),
            Stop::Faster => "faster".into(),
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
        let mut saved = Saved::default();
        let damaged: Vec<String> =
            damaged(&ctx, tmp.path(), &files, &missing_now, &mut saved).await.ok().unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(damaged, ["BravoHotelGame/bad.pak"]);
    }

    #[test]
    fn not_enough_space_never_shows_the_same_two_numbers() {
        const GB: u64 = 1024 * 1024 * 1024;
        // The case from a player: 58.2 GB free, 58.24 GB needed.
        let free = 58 * GB + GB / 5 + GB / 100;
        let needed = 58 * GB + GB / 5 + GB / 25;
        let message = short_of_space(free, needed, "", ".");
        assert!(message.contains("58.2 GB free, 58.3 GB needed"), "{message}");
        assert!(message.contains("Free at least 0.1 GB more."), "{message}");
        let far = short_of_space(10 * GB, 30 * GB, " (why)", " then.");
        assert!(far.contains("10.0 GB free, 30.0 GB needed (why). Free at least 20.0 GB more then."), "{far}");
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
