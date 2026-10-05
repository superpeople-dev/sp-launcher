mod auth;
mod community;
mod config;
mod crash_reporter;
mod client_fixes;
mod client_fixes_deployment;
mod client_fixes_startup;
mod discord;
mod download;
mod engine_ini;
mod error;
mod game;
mod hardware;
mod integrity;
mod pcid;
mod replays;
mod reports;
mod shim;
mod startup_images;
mod twitch;
mod gateway;
mod leaderboard;
mod smart_app_control;
pub mod hosts;
pub mod news;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use config::Config;
use error::{LauncherError, Result};

/// Everything the commands share. Plain mutexes: the contents are tiny and
/// only touched on user actions.
pub struct AppState {
    config_dir: PathBuf,
    config: Mutex<Config>,
    /// True while the game is running and our hosts block is in place.
    redirect_active: Arc<AtomicBool>,
    /// Set while the game is running, so `stop_game` knows what to kill.
    running_pid: Arc<Mutex<Option<u32>>>,
    /// Discord rich presence. A handle to a worker thread, so nothing here
    /// ever waits on Discord.
    discord: discord::Presence,
    /// The Download tab's worker state (download.rs).
    download: download::Downloader,
    /// While the Discord window is open: where its outcome goes (the one-time
    /// code, or why there is none). See `discord_connect`.
    signing_in: Arc<Mutex<Option<SignIn>>>,
    /// The platforms and types an idea is filed under, from the last page load.
    meta: Mutex<community::Meta>,
    /// A reported match's link (sp-launcher://replay/...) waiting for the
    /// admin's yes (replays.rs). Kept here so a link that started the launcher
    /// is still there when its page has loaded.
    replay_link: Mutex<Option<replays::Link>>,
}

type SignIn = tokio::sync::oneshot::Sender<std::result::Result<String, String>>;

/// `Window` (handed to `on_window_event`) and `WebviewWindow` (handed back
/// by `get_webview_window`) have identical hide/show methods but no shared
/// trait for them, so this is the thin common interface that lets
/// `hide_to_tray_window`/`show_from_tray_window` work from either call site.
trait WindowLike {
    fn hide(&self) -> tauri::Result<()>;
    fn show(&self) -> tauri::Result<()>;
    fn set_focus(&self) -> tauri::Result<()>;
    fn set_skip_taskbar(&self, skip: bool) -> tauri::Result<()>;
}

impl<R: tauri::Runtime> WindowLike for tauri::Window<R> {
    fn hide(&self) -> tauri::Result<()> {
        tauri::Window::hide(self)
    }
    fn show(&self) -> tauri::Result<()> {
        tauri::Window::show(self)
    }
    fn set_focus(&self) -> tauri::Result<()> {
        tauri::Window::set_focus(self)
    }
    fn set_skip_taskbar(&self, skip: bool) -> tauri::Result<()> {
        tauri::Window::set_skip_taskbar(self, skip)
    }
}

impl<R: tauri::Runtime> WindowLike for tauri::WebviewWindow<R> {
    fn hide(&self) -> tauri::Result<()> {
        tauri::WebviewWindow::hide(self)
    }
    fn show(&self) -> tauri::Result<()> {
        tauri::WebviewWindow::show(self)
    }
    fn set_focus(&self) -> tauri::Result<()> {
        tauri::WebviewWindow::set_focus(self)
    }
    fn set_skip_taskbar(&self, skip: bool) -> tauri::Result<()> {
        tauri::WebviewWindow::set_skip_taskbar(self, skip)
    }
}

/// Hide the window and drop it from the taskbar, leaving the tray icon as
/// the only way back. Used by the minimize/close buttons and by a
/// CloseRequested from the OS (Alt+F4 etc.) alike, so there's one consistent
/// "hidden" state instead of minimize and close behaving differently.
fn hide_to_tray_window(window: &impl WindowLike) {
    let _ = window.hide();
    let _ = window.set_skip_taskbar(true);
}

/// Undo `hide_to_tray_window`: restore the taskbar entry, show, and focus.
fn show_from_tray_window(window: &impl WindowLike) {
    let _ = window.set_skip_taskbar(false);
    let _ = window.show();
    let _ = window.set_focus();
}

// ---------------------------------------------------------------- config ---

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Config {
    state.config.lock().expect("config mutex").clone()
}

#[tauri::command]
fn set_config(state: State<'_, AppState>, cfg: Config) -> Result<()> {
    let mut current = state.config.lock().expect("config mutex");
    // The sign-in belongs to this side (discord_connect, sign_out): the
    // frontend's copy of the config may be older than a sign-in, and must not
    // put an old one back or drop a new one.
    let cfg = Config {
        session_sealed: current.session_sealed.clone(),
        profile: current.profile.clone(),
        device_id: current.device_id.clone(),
        last_version: current.last_version.clone(),
        auth_key_sealed: current.auth_key_sealed.clone(),
        account_id: current.account_id.clone(),
        display_name: current.display_name.clone(),
        key_status: current.key_status.clone(),
        // Client fixes are always on (the console lock and the login ticket
        // need them); an older frontend's switch cannot turn them off.
        client_fixes_enabled: true,
        ..cfg
    };
    config::save(&state.config_dir, &cfg)?;
    *current = cfg;
    Ok(())
}

// --------------------------------------------------------------- install ---

#[tauri::command]
fn install_state(state: State<'_, AppState>) -> game::InstallState {
    let dir = state.config.lock().expect("config mutex").install_dir.clone();
    game::detect(&dir)
}

/// Play's check of the game's files (integrity.rs), for the UI.
#[derive(Serialize, Clone)]
struct GameFiles {
    ok: bool,
    message: String,
    missing: usize,
    changed: usize,
    unchecked: usize,
    extra: Vec<String>,
}

impl From<integrity::Report> for GameFiles {
    fn from(r: integrity::Report) -> Self {
        GameFiles { ok: r.ok(), message: r.message(), missing: r.missing.len(), changed: r.changed.len(), unchecked: r.unchecked, extra: r.extra }
    }
}

/// The Game folder against the website's list and what the launcher checked.
async fn game_files(state: &AppState) -> Result<integrity::Report> {
    let dir = state.config.lock().expect("config mutex").install_dir.trim().to_string();
    let config_dir = state.config_dir.clone();
    let official = download::official_files()
        .await
        .map_err(|e| LauncherError::Message(format!("Could not check the game's files against superpeople.dev: {e}")))?;
    tauri::async_runtime::spawn_blocking(move || integrity::check(&config_dir, std::path::Path::new(&dir), &official))
        .await
        .map_err(|e| LauncherError::Message(e.to_string()))
}

/// What the Play button shows: Play, or Verify files when the files do not match.
#[tauri::command]
async fn check_game_files(state: State<'_, AppState>) -> Result<GameFiles> {
    Ok(game_files(&state).await?.into())
}

/// Windows Security on App & browser control, for the Smart App Control
/// window (smart_app_control.rs).
#[tauri::command]
fn open_windows_security() -> Result<()> {
    if smart_app_control::open_settings() {
        Ok(())
    } else {
        Err(LauncherError::Message("Windows Security could not be opened. Open it from the Start menu.".into()))
    }
}

#[tauri::command]
fn open_install_dir(state: State<'_, AppState>) -> Result<()> {
    let dir = state.config.lock().expect("config mutex").install_dir.clone();
    if dir.is_empty() {
        return Err(LauncherError::Message("no install directory set".into()));
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(&dir).spawn()?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open").arg(&dir).spawn()?;
    }
    Ok(())
}

// ------------------------------------------------------------------ news ---

#[tauri::command]
async fn fetch_news(state: State<'_, AppState>) -> Result<Vec<news::NewsItem>> {
    let _ = state;
    news::fetch(news::FEED_URL).await
}

// ----------------------------------------------------------------- hosts ---

#[tauri::command]
fn hosts_status(_state: State<'_, AppState>) -> hosts::HostsStatus {
    hosts::status(&hosts::domains())
}

#[tauri::command]
fn hosts_apply(state: State<'_, AppState>) -> Result<()> {
    hosts::ensure(&state.config_dir)?;
    state.redirect_active.store(true, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
fn hosts_remove(state: State<'_, AppState>) -> Result<()> {
    hosts::remove_elevated(&state.config_dir)?;
    state.redirect_active.store(false, Ordering::Relaxed);
    Ok(())
}

/// Relaunch the launcher with elevation. Uses PowerShell's Start-Process
/// rather than pulling in the Windows API crate for one call; the UAC prompt
/// is identical either way.
#[tauri::command]
fn relaunch_elevated(app: AppHandle) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let exe = std::env::current_exe()?;
        let path = exe.display().to_string();
        if path.contains('\'') {
            return Err(LauncherError::Message(
                "cannot elevate: the launcher path contains a quote".into(),
            ));
        }
        // CREATE_NO_WINDOW: -WindowStyle Hidden only hides the console once PowerShell has
        // started, so without it a blue PowerShell window flashed up while the launcher opened.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &format!("Start-Process -FilePath '{path}' -Verb RunAs"),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| LauncherError::Message(format!("could not request elevation: {e}")))?;
        app.exit(0);
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Err(LauncherError::Message(
            "elevation is only meaningful on Windows".into(),
        ))
    }
}

// ------------------------------------------------------------------ tray ---

/// Called by the minimize and close title-bar buttons: send the window to
/// the tray instead of the taskbar or actually quitting.
#[tauri::command]
fn hide_to_tray(app: AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        hide_to_tray_window(&window);
    }
    Ok(())
}

// ------------------------------------------------------------------ auth ---
//
// The session never leaves this side except to go to the website. It is not
// returned to the frontend, not logged, and not passed to the game -- what the
// game gets is the backend's one-time ticket, minted at launch.

/// Makes sure this installation has an id, persisting it the first time.
fn ensure_device_id(state: &State<'_, AppState>) -> Result<String> {
    {
        let cfg = state.config.lock().expect("config mutex");
        if !cfg.device_id.is_empty() {
            return Ok(cfg.device_id.clone());
        }
    }
    let id = auth::new_device_id();
    {
        let mut cfg = state.config.lock().expect("config mutex");
        cfg.device_id = id.clone();
        config::save(&state.config_dir, &cfg)?;
    }
    Ok(id)
}

/// The saved session, if the player is signed in.
fn session_of(state: &State<'_, AppState>) -> Result<Option<String>> {
    let sealed = state.config.lock().expect("config mutex").session_sealed.clone();
    if sealed.is_empty() {
        return Ok(None);
    }
    auth::unseal(&sealed).map(Some)
}

fn require_session(state: &State<'_, AppState>) -> Result<String> {
    session_of(state)?.ok_or(LauncherError::SignedOut)
}

/// Forgets the sign-in on this PC. Also what happens when the website stops
/// accepting it (`expired`).
fn forget_sign_in(state: &State<'_, AppState>) -> Result<()> {
    let mut cfg = state.config.lock().expect("config mutex");
    cfg.session_sealed.clear();
    cfg.profile = None;
    // What a launcher key left behind goes too. device_id deliberately
    // survives: it identifies the installation, not the player.
    cfg.auth_key_sealed.clear();
    cfg.account_id.clear();
    cfg.display_name.clear();
    cfg.key_status.clear();
    config::save(&state.config_dir, &cfg)
}

/// Passes an error through; the website refusing the session also signs the
/// launcher out and sends it back to the welcome screen.
fn expired(app: &AppHandle, state: &State<'_, AppState>, error: LauncherError) -> LauncherError {
    if matches!(error, LauncherError::SignedOut) {
        let _ = forget_sign_in(state);
        let _ = app.emit("auth:expired", ());
    }
    error
}

/// The player's in-game name and when they may change it next; with `name`,
/// changes it (auth::game_name).
#[tauri::command]
async fn account_name(app: AppHandle, state: State<'_, AppState>, name: Option<String>) -> Result<auth::GameName> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    auth::game_name(&session, name.as_deref()).await.map_err(|e| update_first(&app, expired(&app, &state, e)))
}

/// The season's top 100 of each mode (leaderboard.rs); None while the backend
/// has none.
#[tauri::command]
async fn leaderboard() -> Result<Option<leaderboard::Board>> {
    leaderboard::load().await
}

/// The region the player's matches are in, and the regions with servers now;
/// with `region`, picks it (auth::game_region).
#[tauri::command]
async fn account_region(app: AppHandle, state: State<'_, AppState>, region: Option<String>) -> Result<auth::GameRegion> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    auth::game_region(&session, region.as_deref()).await.map_err(|e| update_first(&app, expired(&app, &state, e)))
}

/// Who is live on Twitch in the SUPER PEOPLE category (twitch.rs).
#[tauri::command]
async fn twitch_streams() -> Result<twitch::Streams> {
    twitch::live().await
}

#[tauri::command]
fn auth_status(state: State<'_, AppState>) -> auth::AuthState {
    let cfg = state.config.lock().expect("config mutex");
    auth::AuthState {
        profile: if cfg.session_sealed.is_empty() { None } else { cfg.profile.clone() },
    }
}

/// "Connect with Discord": opens the website's launcher sign-in in a window of
/// its own and waits until the player is through, or closes it. The site ends
/// on /launcher/connected?code=…; that address is caught here, never loaded,
/// and the code traded for the session (auth.rs).
///
/// The window is a plain web page with no access to the launcher: the app's
/// capabilities (capabilities/default.json) are for the "main" window only.
#[tauri::command]
async fn discord_connect(app: AppHandle, state: State<'_, AppState>) -> Result<auth::Profile> {
    let pkce = auth::pkce()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    // A new attempt replaces an old one; its waiter hears "cancelled".
    if let Some(old) = state.signing_in.lock().expect("sign-in mutex").replace(tx) {
        let _ = old.send(Err("cancelled".into()));
    }
    if let Some(open) = app.get_webview_window(DISCORD_WINDOW) {
        let _ = open.destroy();
    }

    let sign_in = auth::sign_in_url(&pkce.challenge);
    // Never a window with an error page in it: a site that cannot sign anyone
    // in right now is said on the welcome screen instead.
    if let Err(e) = auth::check_sign_in(&sign_in).await {
        state.signing_in.lock().expect("sign-in mutex").take();
        return Err(e);
    }
    let url: tauri::Url = sign_in.parse().map_err(|_| LauncherError::Message(auth::OOPS.into()))?;
    let slot = state.signing_in.clone();
    let mut builder = tauri::WebviewWindowBuilder::new(&app, DISCORD_WINDOW, tauri::WebviewUrl::External(url))
        .title("Connect with Discord")
        .inner_size(500.0, 760.0)
        .resizable(false)
        .center()
        // Private, like an incognito tab: nothing from an earlier sign-in is
        // remembered, so each one asks for a Discord login (or the Discord
        // app's approval) afresh, and no Discord session stays on the PC.
        .incognito(true)
        .on_navigation(move |url| match auth::read_connected(url.as_str()) {
            Some(outcome) => {
                if let Some(tx) = slot.lock().expect("sign-in mutex").take() {
                    let _ = tx.send(outcome);
                }
                false
            }
            None => true,
        });
    if let Some(main) = app.get_webview_window("main") {
        builder = builder.parent(&main).map_err(|e| LauncherError::Message(e.to_string()))?;
    }
    let window = builder.build().map_err(|_| LauncherError::Message(auth::OOPS.into()))?;
    let slot = state.signing_in.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            if let Some(tx) = slot.lock().expect("sign-in mutex").take() {
                let _ = tx.send(Err("cancelled".into()));
            }
        }
    });

    let outcome = rx.await.unwrap_or_else(|_| Err("cancelled".into()));
    if let Some(open) = app.get_webview_window(DISCORD_WINDOW) {
        let _ = open.destroy();
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_focus();
    }
    let code = outcome.map_err(LauncherError::Message)?;

    let (token, profile) = auth::exchange(&code, &pkce.verifier).await?;
    let sealed = auth::seal(&token)?;
    forget_sign_in(&state)?;
    {
        let mut cfg = state.config.lock().expect("config mutex");
        cfg.session_sealed = sealed;
        cfg.profile = Some(profile.clone());
        config::save(&state.config_dir, &cfg)?;
    }
    note_version(&state, token);
    Ok(profile)
}

const DISCORD_WINDOW: &str = "discord";

/// The welcome screen's Cancel: closes the Discord window, which ends the wait.
#[tauri::command]
fn discord_cancel(app: AppHandle, state: State<'_, AppState>) {
    if let Some(tx) = state.signing_in.lock().expect("sign-in mutex").take() {
        let _ = tx.send(Err("cancelled".into()));
    }
    if let Some(open) = app.get_webview_window(DISCORD_WINDOW) {
        let _ = open.destroy();
    }
}

#[tauri::command]
async fn sign_out(state: State<'_, AppState>) -> Result<()> {
    // #discord-auth-logs hears it first, while the session still works.
    if let Ok(Some(session)) = session_of(&state) {
        auth::signed_out(&session).await;
    }
    forget_sign_in(&state)
}

/// The profile as the website sees it now (name, picture, admin rights), saved
/// for the next start. Called in the background when the launcher opens.
#[tauri::command]
async fn auth_refresh(app: AppHandle, state: State<'_, AppState>) -> Result<Option<auth::Profile>> {
    let Some(session) = session_of(&state)? else { return Ok(None) };
    let profile = auth::me(&session).await.map_err(|e| expired(&app, &state, e))?;
    {
        let mut cfg = state.config.lock().expect("config mutex");
        if !cfg.session_sealed.is_empty() {
            cfg.profile = Some(profile.clone());
            config::save(&state.config_dir, &cfg)?;
        }
    }
    note_version(&state, session);
    Ok(Some(profile))
}

/// Whether the signed-in player may play, from the website (auth.rs `Ban`).
/// None: they may. A ban until lifted signs the launcher out here and now (the
/// website will not let them sign in again); the UI shows why on the welcome
/// screen. A temporary ban stays on the Play page. Called when the launcher
/// opens, when it comes back to the front, before Play, and every minute while
/// the game runs: the UI closes a game whose player is banned once they are
/// out of their match.
#[tauri::command]
async fn ban_status(app: AppHandle, state: State<'_, AppState>) -> Result<Option<auth::Ban>> {
    let Some(session) = session_of(&state)? else { return Ok(None) };
    let (_, ban) = auth::account(&session).await.map_err(|e| expired(&app, &state, e))?;
    if ban.as_ref().is_some_and(|b| b.permanent) {
        forget_sign_in(&state)?;
    }
    Ok(ban)
}

/// #launcher-logs: "Launcher updated, v0.9.1 to v0.9.2", once per update, the
/// first time the new version runs while signed in (the website needs the
/// session to say who). The first version that knows this only remembers
/// itself: from an older one, there is nothing to compare with.
fn note_version(state: &State<'_, AppState>, session: String) {
    let now = env!("CARGO_PKG_VERSION");
    let before = {
        let mut cfg = state.config.lock().expect("config mutex");
        if cfg.last_version == now {
            return;
        }
        let before = std::mem::replace(&mut cfg.last_version, now.to_string());
        if config::save(&state.config_dir, &cfg).is_err() {
            return;
        }
        before
    };
    if !before.is_empty() {
        // With the PC it runs on (hardware.rs), as for "Game launched".
        tauri::async_runtime::spawn(async move {
            let pc = tauri::async_runtime::spawn_blocking(hardware::summary).await.unwrap_or_default();
            auth::report(session, serde_json::json!({ "action": "launcher.updated", "from": before, "hardware": pc }));
        });
    }
}

// ----------------------------------------------------------------- terms ---
// Play needs the Terms of Service accepted (auth.rs terms).

/// The current terms, and whether this Discord account accepted them.
#[tauri::command]
async fn launcher_terms(app: AppHandle, state: State<'_, AppState>) -> Result<auth::Terms> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    auth::terms(&session).await.map_err(|e| expired(&app, &state, e))
}

/// The player read the terms of `version` and accepted them. Answers with the
/// terms as they are now: accepted, or, if they changed while the player read
/// them, the new ones to read.
#[tauri::command]
async fn accept_terms(app: AppHandle, state: State<'_, AppState>, version: String) -> Result<auth::Terms> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    match auth::accept_terms(&session, &version).await {
        Ok(()) | Err(LauncherError::TermsRequired) => {}
        Err(e) => return Err(expired(&app, &state, e)),
    }
    auth::terms(&session).await.map_err(|e| expired(&app, &state, e))
}

/// Passes an error through; a "terms first" also has the UI open them.
fn terms_first(app: &AppHandle, error: LauncherError) -> LauncherError {
    if matches!(error, LauncherError::TermsRequired) {
        let _ = app.emit("terms:required", ());
    }
    update_first(app, error)
}

/// Passes an error through; "too old a launcher" also has the UI install the
/// update (App.tsx, launcher:outdated).
fn update_first(app: &AppHandle, error: LauncherError) -> LauncherError {
    if matches!(error, LauncherError::UpdateRequired) {
        let _ = app.emit("launcher:outdated", ());
    }
    error
}

// ------------------------------------------------------------- community ---
// The Ideas, Roadmap and Completed pages (community.rs).

#[tauri::command]
async fn community_items(app: AppHandle, state: State<'_, AppState>, board: String) -> Result<Vec<community::Item>> {
    let session = session_of(&state)?;
    let (items, meta) = community::items(session.as_deref(), &board).await.map_err(|e| expired(&app, &state, e))?;
    if !meta.platforms.is_empty() || !meta.types.is_empty() {
        *state.meta.lock().expect("meta mutex") = meta;
    }
    Ok(items)
}

/// The platforms and types an idea can be filed under, loading them if no
/// page has yet.
#[tauri::command]
async fn community_meta(app: AppHandle, state: State<'_, AppState>) -> Result<community::Meta> {
    let known = state.meta.lock().expect("meta mutex").clone();
    if !known.platforms.is_empty() {
        return Ok(known);
    }
    let session = session_of(&state)?;
    let (_, meta) = community::items(session.as_deref(), "ideas").await.map_err(|e| expired(&app, &state, e))?;
    *state.meta.lock().expect("meta mutex") = meta.clone();
    Ok(meta)
}

#[tauri::command]
async fn community_thread(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<community::Thread> {
    let session = session_of(&state)?;
    community::thread(session.as_deref(), &id).await.map_err(|e| expired(&app, &state, e))
}

#[tauri::command]
async fn community_vote(app: AppHandle, state: State<'_, AppState>, id: String, direction: String) -> Result<community::VoteResult> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    community::vote(&session, &id, &direction).await.map_err(|e| expired(&app, &state, e))
}

#[tauri::command]
async fn community_comment(app: AppHandle, state: State<'_, AppState>, id: String, body: String) -> Result<community::Comment> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    community::comment(&session, &id, &body).await.map_err(|e| expired(&app, &state, e))
}

#[tauri::command]
async fn community_post_idea(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    description: String,
    kind: String,
    platform: String,
) -> Result<()> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    community::post_idea(&session, &title, &description, &kind, &platform)
        .await
        .map_err(|e| expired(&app, &state, e))
}

/// An admin's change to an item (community::admin). The website decides
/// whether this player may make it.
#[tauri::command]
async fn community_admin(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    change: serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    community::admin(&session, &id, change).await.map_err(|e| expired(&app, &state, e))
}

#[tauri::command]
async fn community_comments_off(app: AppHandle, state: State<'_, AppState>, id: String, off: bool) -> Result<()> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    community::comments_off(&session, &id, off).await.map_err(|e| expired(&app, &state, e))
}

#[tauri::command]
async fn community_delete_comment(app: AppHandle, state: State<'_, AppState>, id: String, comment_id: String) -> Result<()> {
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    community::delete_comment(&session, &id, &comment_id).await.map_err(|e| expired(&app, &state, e))
}

// ---------------------------------------------------------------- launch ---

#[derive(Serialize)]
struct LaunchResult {
    pid: u32,
    redirected: bool,
}

#[tauri::command]
async fn launch_game(
    app: AppHandle,
    state: State<'_, AppState>,
    server: Option<String>,
) -> Result<LaunchResult> {
    let cfg = state.config.lock().expect("config mutex").clone();
    let config_dir = state.config_dir.clone();

    // Get the login ticket FIRST, before anything with a side effect.
    //
    // It is the step most likely to fail -- signed out, account suspended,
    // website or backend down -- and failing here leaves the machine completely
    // untouched: no Engine.ini edit, no hosts entries, no process. It also means
    // the game is never started in a state where it cannot log in.
    //
    // The website gives this Discord session a two-minute game pass; the
    // backend checks it, finds the account, and answers with the ticket (and
    // remembers this launch for the game's own login). See auth.rs.
    let mut env: Vec<(String, String)> = Vec::new();
    let session = require_session(&state).map_err(|e| expired(&app, &state, e))?;
    // The Terms of Service, asked here as well as by the Play button: the
    // button only knows what the website said when the launcher last asked.
    let terms = auth::terms(&session).await.map_err(|e| expired(&app, &state, e))?;
    if !terms.accepted {
        return Err(terms_first(&app, LauncherError::TermsRequired));
    }
    // The game's files are the official ones (integrity.rs): otherwise no Play,
    // and the button becomes Verify files.
    let files = game_files(&state).await?;
    if !files.ok() {
        let message = files.message();
        let _ = app.emit("files:changed", GameFiles::from(files));
        return Err(LauncherError::Message(message));
    }
    {
        let device_id = ensure_device_id(&state)?;
        // This PC's one-way code (pcid.rs), so a ban follows the PC. Read
        // once per run, off the async threads: it asks the firmware and disk.
        let pc = tauri::async_runtime::spawn_blocking(pcid::codes)
            .await
            .unwrap_or_default();
        let pass = auth::game_pass(&session).await.map_err(|e| terms_first(&app, expired(&app, &state, e)))?;
        let ticket = auth::discord_launch(&pass, &device_id, &pc).await.map_err(|e| update_first(&app, e))?;
        if cfg.debug_logging {
            // The lifetime, never the token. A ticket in a log file is a ticket
            // someone else can use for the next minute.
            eprintln!("[auth] login ticket minted, valid for {}s", ticket.expires_in);
        }
        // Environment, not argv: see the note on LaunchSpec::env.
        env.push(("SP_AUTH_TICKET".into(), ticket.token));
    }
    // The phase from the region the backend has for this player: the Dev region
    // (staff and invited players) starts the game with the dev lobby, every
    // other region with the live one (game::phase_for_region). Asked here, with
    // the other checks, before anything with a side effect.
    let region = auth::game_region(&session, None).await.map_err(|e| expired(&app, &state, e))?;
    let phase = game::phase_for_region(&region.region);

    // Prepare optional session-owned fixes before starting the game. Early
    // failures roll deployment back through the session guard.
    let mut fixes_session = client_fixes::prepare(&cfg.install_dir, &config_dir, cfg.client_fixes_enabled)?;

    match shim::apply(&cfg.install_dir) {
        Ok(shim::Applied::NotBundled) => {
            if cfg.client_fixes_enabled {
                return Err(LauncherError::Message("This launcher is missing the proxy required to load Client fixes. Download a complete build.".into()));
            }
            eprintln!("[shim] this launcher has no DLL bundled -- the game needs XAPOFX1_5.dll placed by hand");
        }
        Ok(what) => {
            if cfg.debug_logging {
                eprintln!("[shim] no-Steam DLL: {what:?}");
            }
        }
        Err(e) => return Err(e),
    }

    // Windows 11's Smart App Control refuses DLLs without a trusted signature,
    // and ours are not code-signed yet: the game would stop at start with "Bad
    // Image" (0xc0e90002). Say so instead (smart_app_control.rs); the fixes
    // deployment is rolled back as for any early failure.
    if let Some(file) = smart_app_control::blocked(&cfg.install_dir, cfg.client_fixes_enabled) {
        eprintln!("[smart app control] on, and {} is not signed", file.display());
        let _ = app.emit("windows:smart-app-control", ());
        return Err(LauncherError::SmartAppControl);
    }

    // The community's startup pictures (startup_images.rs). A picture that
    // cannot be written is never a reason not to start the game.
    if let Err(e) = startup_images::apply(&cfg.install_dir) {
        eprintln!("[startup images] {e}");
    }
    // The crash window's Send goes to the backend and the staff's #crash-logs
    // (crash_reporter.rs), not the original developer's BugSplat. Never a
    // reason not to start the game either.
    if let Err(e) = crash_reporter::apply() {
        eprintln!("[crash reporter] {e}");
    }

    // Explicitly override inherited settings even for disabled launches. The debug
    // window is for the staff only (admins, moderators, developers): a player who
    // turned it on before it was hidden from them does not get it.
    let debug_window = cfg.client_fixes_debug_window && cfg.profile.as_ref().is_some_and(auth::Profile::is_staff);
    env.extend(client_fixes_startup::environment(cfg.client_fixes_enabled, debug_window));
    let fixes_startup = if cfg.client_fixes_enabled {
        Some(client_fixes_startup::Startup::new()?)
    } else { None };
    if let Some(startup) = &fixes_startup {
        env.extend(startup.environment());
    }
    // The game's Report button (reports.rs): the client fixes DLL writes each
    // report into this folder, and the launcher sends it to the staff. Without
    // client fixes nothing writes there, so no folder is named.
    let reports_dir = if cfg.client_fixes_enabled { reports::prepare(&config_dir) } else { None };
    if let Some(dir) = &reports_dir {
        env.push((reports::ENV.into(), dir.display().to_string()));
    }

    engine_ini::apply()?;

    // Redirect first: the game reads the hostnames on startup, so the entries
    // have to be in place before the process exists, not just before it
    // connects.
    // Since 0.3.0 the entries are permanent: after the first start this is a
    // read-only check, and only a missing/outdated block costs a UAC prompt.
    // Nothing is removed when the game exits.
    let redirected = if cfg.hosts_redirect {
        let dir = config_dir.clone();
        tauri::async_runtime::spawn_blocking(move || hosts::ensure(&dir))
            .await
            .map_err(|e| LauncherError::Message(e.to_string()))??;
        state.redirect_active.store(true, Ordering::Relaxed);
        true
    } else {
        false
    };

    // The game's console stays on only for an admin, with a token the website
    // signed (auth::console_token; the client fixes DLL checks it). Asked for
    // right before the start: it is good for two minutes. Never a reason not to
    // start: without it the console is simply off.
    if cfg.profile.as_ref().is_some_and(|p| p.admin) {
        match auth::console_token(&session).await {
            Some(token) => env.push(("SP_CONSOLE_PASS".into(), token)),
            None => eprintln!("[console] no console token from the website -- the game's console stays off"),
        }
    }

    let mut child = game::launch(game::LaunchSpec {
        install_dir: &cfg.install_dir,
        server: server.as_deref(),
        phase,
        user_args: &cfg.launch_args,
        env: &env,
    })?;

    let pid = child.id();
    if let Err(error) = fixes_session.mark_running(pid) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    if let Some(startup) = fixes_startup {
        // Antivirus that takes the proxy away right after shim::apply wrote it leaves the game
        // with Windows' own XAPOFX1_5.dll: no Client fixes, and an end on the PAK's signature
        // (client_fixes_startup::proxy_removed).
        let proxy = shim::target_path(std::path::Path::new(&cfg.install_dir));
        let (returned_child, result) = tauri::async_runtime::spawn_blocking(move || {
            let result = startup.wait(&mut child, &|| proxy.is_file());
            (child, result)
        }).await.map_err(|e| LauncherError::Message(format!("Client fixes startup task failed: {e}")))?;
        child = returned_child;
        if let Err(error) = result {
            if let Err(kill_error) = child.kill() {
                return Err(LauncherError::Message(format!(
                    "{error}; also could not stop the game after Client fixes startup failed: {kill_error}"
                )));
            }
            let _ = child.wait();
            return Err(error);
        }
    }
    *state.running_pid.lock().expect("pid mutex") = Some(pid);
    state.discord.set(discord::State::InGame);

    // Reports made with the game's Report button go to the staff (reports.rs):
    // every few seconds while this game runs, and once more after it closes.
    // With each goes the replay of its match (replays.rs), once the match is over.
    if let Some(dir) = reports_dir {
        let running_pid = state.running_pid.clone();
        let session = session.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(reports::EVERY).await;
                let closed = running_pid.lock().map(|running| *running != Some(pid)).unwrap_or(true);
                reports::send_pending(&session, &dir, !closed).await;
                reports::send_tamper(&session, &dir).await;
                if closed {
                    break;
                }
            }
        });
    }

    // #launcher-logs: who started the game, on what PC (hardware.rs).
    tauri::async_runtime::spawn(async move {
        let pc = tauri::async_runtime::spawn_blocking(hardware::summary).await.unwrap_or_default();
        auth::report(session, serde_json::json!({ "action": "game.launched", "hardware": pc }));
    });

    // The launcher used to hide itself here and only reappear when the game
    // exited. Now it stays open — the Play tab swaps its button for "Close
    // Game" instead — and `close_on_launch` just means "send it to the tray
    // instead", which the tray icon can bring back at any time.
    if cfg.close_on_launch {
        if let Some(window) = app.get_webview_window("main") {
            hide_to_tray_window(&window);
        }
    }

    // Wait for the game, then remove only session-owned client fixes files.
    // The permanent hosts redirect is unchanged.
    // This also fires when `stop_game` kills the process, so that command
    // doesn't need to duplicate any of this cleanup itself.
    {
        let app = app.clone();
        let running_pid = state.running_pid.clone();
        let presence = state.discord.clone();
        let mut child = child;
        tauri::async_runtime::spawn_blocking(move || {
            let status = child.wait();
            if let Err(error) = fixes_session.cleanup() {
                eprintln!("[client fixes] cleanup failed: {error}");
                let _ = app.emit("game:cleanup-failed", error.to_string());
            }
            *running_pid.lock().expect("pid mutex") = None;
            presence.set(discord::State::InLauncher);
            let code = status.ok().and_then(|s| s.code());
            let _ = app.emit("game:exited", code);
            if let Some(window) = app.get_webview_window("main") {
                show_from_tray_window(&window);
            }
        });
    }

    Ok(LaunchResult { pid, redirected })
}

/// Kills the running game. Doesn't touch the hosts redirect or emit
/// `game:exited` itself — the `wait()` in `launch_game`'s background task
/// notices the process die and does all of that, the same as if the game
/// had exited on its own.
#[tauri::command]
fn stop_game(state: State<'_, AppState>) -> Result<()> {
    let pid = *state.running_pid.lock().expect("pid mutex");
    let Some(pid) = pid else {
        return Err(LauncherError::Message("No game is running".into()));
    };

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| LauncherError::Message(format!("could not stop the game: {e}")))?;
        Ok(())
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = pid;
        Err(LauncherError::Message(
            "stopping the game is only implemented on Windows".into(),
        ))
    }
}

// ----------------------------------------------------------- server status ---

/// What the title bar shows: is the backend reachable, and how many play.
#[derive(Serialize)]
struct ServerStatus {
    online: bool,
    players: u32,
}

/// Public `GET /launcher/api/status` (one number, no auth). Any failure --
/// timeout, refused, bad JSON -- simply reads as offline.
#[tauri::command]
async fn server_status() -> ServerStatus {
    let offline = ServerStatus { online: false, players: 0 };
    let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(5)).build() else {
        return offline;
    };
    let url = format!("{}/status", auth::AUTH_BASE_URL);
    let Ok(res) = client.get(url).send().await else { return offline };
    if !res.status().is_success() {
        return offline;
    }
    match res.json::<serde_json::Value>().await {
        Ok(v) if v.get("ok").and_then(|o| o.as_bool()) == Some(true) => ServerStatus {
            online: true,
            players: v.get("players_online").and_then(|n| n.as_u64()).unwrap_or(0) as u32,
        },
        _ => offline,
    }
}

// --------------------------------------------------------------- download ---

#[tauri::command]
fn download_status(state: State<'_, AppState>) -> download::Status {
    state.download.status()
}

#[tauri::command]
fn download_start(app: AppHandle, state: State<'_, AppState>, dir: String, verify: Option<bool>) -> Result<()> {
    let verify = verify.unwrap_or(false);
    download::start(&app, &state.download, dir, verify, require_session(&state)?)
}

#[tauri::command]
fn download_pause(state: State<'_, AppState>) {
    download::pause(&state.download);
}

/// Before installing a launcher update: a running download or Verify files
/// stops where it is and the updated launcher continues it. True if one was
/// running. Async: it waits up to 10 s for the files to close.
#[tauri::command]
async fn download_pause_for_update(state: State<'_, AppState>) -> Result<bool> {
    Ok(download::pause_for_update(&state.download))
}

/// The run a launcher update interrupted, once (the frontend starts it again).
#[tauri::command]
fn download_take_resume(state: State<'_, AppState>) -> Option<download::Resume> {
    download::take_resume(&state.download)
}

#[tauri::command]
async fn download_cancel(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    download::cancel(&app, &state.download)
}

/// Suggested target folder: the game folder from Settings if one is set,
/// else `<system drive>\Games\SUPER PEOPLE`.
#[tauri::command]
fn download_default_dir(state: State<'_, AppState>) -> String {
    let cfg = state.config.lock().expect("config mutex");
    if !cfg.install_dir.is_empty() {
        // One game folder: the Download tab installs into the same folder
        // Settings points at.
        return cfg.install_dir.clone();
    }
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    format!("{drive}\\Games\\SUPER PEOPLE")
}

/// Where the game already is in `dir` (or a few folders below it), if it is:
/// the Download tab then points the Game folder there instead of offering a
/// download. Off the UI thread, since it reads folders.
#[tauri::command]
async fn find_game(dir: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || download::find_game(&dir))
        .await
        .unwrap_or(None)
}

/// What Uninstall would delete from the Game folder (files, bytes), for the
/// uninstall window. Off the UI thread: it walks the whole game.
#[tauri::command]
async fn game_footprint(state: State<'_, AppState>) -> Result<download::Footprint> {
    let dir = state.config.lock().expect("config mutex").install_dir.clone();
    tauri::async_runtime::spawn_blocking(move || download::footprint(&dir))
        .await
        .map_err(|e| LauncherError::Message(e.to_string()))
}

/// The Download tab's Uninstall: deletes the game from the Game folder
/// (download::uninstall says what goes). Not while the game runs, from this
/// launcher or not. Off the UI thread: that is tens of GB of files.
#[tauri::command]
async fn uninstall_game(app: AppHandle) -> Result<()> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let running = state.running_pid.lock().expect("pid mutex").is_some();
        if running || client_fixes_deployment::ensure_no_game_running().is_err() {
            return Err(LauncherError::Message("Close the game before uninstalling it.".into()));
        }
        let dir = state.config.lock().expect("config mutex").install_dir.clone();
        let freed = download::uninstall(&app, &state.download, &dir)?;
        // #launcher-logs, through the website.
        if let Ok(Some(session)) = session_of(&state) {
            auth::report(session, serde_json::json!({ "action": "uninstalled", "files": freed.files, "bytes": freed.bytes }));
        }
        Ok(())
    })
    .await
    .map_err(|e| LauncherError::Message(e.to_string()))?
}

// ------------------------------------------------------------- replays ---
// A reported match opened from its page: sp-launcher://replay/<id>?t=<token>
// (replays.rs). The window asks the admin first (ReplayDialog.tsx), then
// replay_import downloads it with the token and unzips it into the game's
// Demos folder, for the game's Replay menu.

/// A link Windows handed over: kept, the window shown, and told.
fn replay_link_opened(app: &AppHandle, url: &str) {
    let Some(link) = replays::link_of(url) else { return };
    let id = link.id.clone();
    if let Some(state) = app.try_state::<AppState>() {
        *state.replay_link.lock().expect("replay link mutex") = Some(link);
    }
    if let Some(window) = app.get_webview_window("main") {
        show_from_tray_window(&window);
    }
    let _ = app.emit("replay:link", id);
}

/// The replay link waiting for an answer: its id, or None.
#[tauri::command]
fn replay_link(state: State<'_, AppState>) -> Option<String> {
    state.replay_link.lock().expect("replay link mutex").as_ref().map(|l| l.id.clone())
}

/// The admin said no: the link is dropped.
#[tauri::command]
fn replay_dismiss(state: State<'_, AppState>) {
    state.replay_link.lock().expect("replay link mutex").take();
}

/// The admin said yes: the replay, downloaded with the link's token and put in
/// the game's Demos folder.
#[tauri::command]
async fn replay_import(state: State<'_, AppState>) -> Result<replays::Imported> {
    let link = state.replay_link.lock().expect("replay link mutex").take();
    let Some(link) = link else {
        return Err(LauncherError::Message("Open the replay's page again from Discord and press Open in the launcher.".into()));
    };
    let Some(demos) = replays::demos_dir() else {
        return Err(LauncherError::Message("The game's replay folder could not be found on this PC.".into()));
    };
    let bytes = match replays::fetch(&auth::site_url(), auth::AUTH_BASE_URL, &link).await {
        replays::Fetched::Zip(bytes) => bytes,
        replays::Fetched::Expired => {
            return Err(LauncherError::Message(
                "This link was used already or is older than 10 minutes. Open the replay's page again from Discord and press Open in the launcher.".into(),
            ))
        }
        replays::Fetched::Gone => return Err(LauncherError::Message("This replay is no longer on the server: replays are kept 30 days.".into())),
        replays::Fetched::TooBig => return Err(LauncherError::Message("This replay is too big to be one match.".into())),
        replays::Fetched::Failed => {
            return Err(LauncherError::Message("The replay could not be downloaded. Check your connection and open the link again.".into()))
        }
    };
    let imported = tauri::async_runtime::spawn_blocking(move || replays::import(&bytes, &demos))
        .await
        .map_err(|e| LauncherError::Message(e.to_string()))?;
    match imported {
        Ok(done) => Ok(done),
        Err(replays::ImportProblem::NotReplay) => Err(LauncherError::Message("That download is not a game replay. Nothing was added.".into())),
        Err(replays::ImportProblem::TooBig) => Err(LauncherError::Message("This replay is too big to be one match. Nothing was added.".into())),
        Err(replays::ImportProblem::Write) => {
            Err(LauncherError::Message("The replay could not be written to the game's replay folder (Saved\\Demos).".into()))
        }
    }
}

// ------------------------------------------------------------------ entry ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Registered first, as the plugin requires: a second launch hands its
        // arguments to the running instance and exits. Since closing the
        // window only hides it to the tray, "already running" is usually
        // invisible — so bring that window back rather than doing nothing,
        // which would look like the launcher refusing to start.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                show_from_tray_window(&window);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let cfg = config::load(&config_dir);

            // If a previous run was killed with the redirect applied, clean it
            // up before the user can do anything else.
            if let Some(note) = hosts::recover(&config_dir) {
                let _ = app.handle().emit("hosts:recovered", note);
            }

            let downloader = download::Downloader::new(config_dir.clone());
            app.manage(AppState {
                config_dir,
                config: Mutex::new(cfg),
                redirect_active: Arc::new(AtomicBool::new(false)),
                running_pid: Arc::new(Mutex::new(None)),
                discord: discord::Presence::start(),
                download: downloader,
                signing_in: Arc::new(Mutex::new(None)),
                meta: Mutex::new(community::Meta::default()),
                replay_link: Mutex::new(None),
            });

            // sp-launcher:// links: a reported match's "Open in the launcher"
            // (replays.rs). Registered with Windows on every start too, not
            // only by the installer, so a launcher that updated itself or runs
            // from elsewhere still gets them. One that is running already gets
            // them from the next launch (single-instance), one that is not
            // started with it (get_current).
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                #[cfg(windows)]
                let _ = app.deep_link().register_all();
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        replay_link_opened(&handle, url.as_str());
                    }
                });
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        replay_link_opened(app.handle(), url.as_str());
                    }
                }
            }

            // Tray icon: reuses the app's own bundled icon rather than
            // shipping a second asset. "Open" undoes hide_to_tray_window;
            // "Quit" is now the only real way to end the process, since
            // closing the window itself just hides it.
            let show_item = MenuItem::with_id(app, "show", "Open SP Launcher", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            // Not `default_window_icon()`: the app icon keeps the whole mark,
            // wings included, which is 2.19:1 — in a square tray cell that
            // leaves it spanning the full width but under half the height, so
            // it reads as tiny next to everything else in the tray. tray.png
            // is the same artwork cropped to a squarer region, so it fills the
            // cell properly. Only the tray uses it; the installer, taskbar and
            // window icon still get the full logo.
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

            TrayIconBuilder::new()
                .icon(tray_icon)
                .tooltip("SP Launcher")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            show_from_tray_window(&window);
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            show_from_tray_window(&window);
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| match event {
            // The X button and Alt+F4 both raise this. Hide to the tray
            // instead of letting the app quit — Destroyed (below) is now
            // reserved for an actual exit via the tray's Quit item.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide_to_tray_window(window);
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            replay_link,
            replay_dismiss,
            replay_import,
            get_config,
            set_config,
            install_state,
            check_game_files,
            open_windows_security,
            open_install_dir,
            fetch_news,
            hosts_status,
            hosts_apply,
            hosts_remove,
            relaunch_elevated,
            hide_to_tray,
            launch_game,
            stop_game,
            auth_status,
            account_name,
            account_region,
            discord_connect,
            discord_cancel,
            sign_out,
            community_items,
            community_meta,
            community_thread,
            community_admin,
            community_comments_off,
            community_delete_comment,
            auth_refresh,
            ban_status,
            launcher_terms,
            accept_terms,
            community_vote,
            community_comment,
            community_post_idea,
            server_status,
            twitch_streams,
            leaderboard,
            download_status,
            download_start,
            download_pause,
            download_pause_for_update,
            download_take_resume,
            download_cancel,
            download_default_dir,
            find_game,
            game_footprint,
            uninstall_game,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
