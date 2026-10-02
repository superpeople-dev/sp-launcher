//! Signing in with Discord, through superpeople.dev, and what Play needs from it.
//!
//! THE SHAPE OF THE THING
//! ---------------------
//! The player presses "Connect with Discord". The launcher opens the website's
//! launcher sign-in in a window of its own (lib.rs `discord_connect`); the site
//! sends them through Discord and ends on `/launcher/connected?code=…`. The
//! launcher reads that one-time code from the window's address and trades it,
//! with the PKCE verifier it made up front, for the player's session: the same
//! signed session the website uses, 30 days long (sp-website lib/launcher.ts).
//!
//! That session is stored encrypted at rest with Windows DPAPI. It is what the
//! launcher shows Ideas/Roadmap/Completed with, votes and comments with
//! (community.rs), and what it asks the site for a GAME PASS with when Play is
//! pressed: two minutes, one use, signed by the site. The game backend checks
//! the pass and lets that Discord account's game account in. With the pass
//! goes a one-way code of this PC (pcid.rs), so a ban can follow the PC.
//!
//! WHY IT IS BUILT THIS WAY
//! ------------------------
//! Everything on the player's machine is attacker-controlled, so nothing here
//! ever asserts who the player is. Discord tells the website, the website signs
//! it, and the backend only believes the website's signature. A Discord id
//! typed or pasted anywhere on this side is worth nothing. The PKCE verifier
//! makes the one-time code useless to anything else that sees the address.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{LauncherError, Result};
use crate::pcid;

/// Where the game backend lives. Fixed, like `news::FEED_URL`: it is this
/// server's API, and a player pointing the launcher elsewhere only breaks
/// their own login. The port must match `http.port` in the backend's config.
pub const AUTH_BASE_URL: &str = "http://64.226.112.204:8080/launcher/api";

/// The website players sign in with.
pub const SITE_URL: &str = "https://superpeople.dev";

/// The site, or in a debug build a local copy of it (`SP_SITE_URL`, e.g.
/// http://localhost:3000, the other address Discord accepts). A release build
/// only ever talks to superpeople.dev.
pub fn site_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("SP_SITE_URL") {
        if !url.trim().is_empty() {
            return url.trim().trim_end_matches('/').to_string();
        }
    }
    SITE_URL.to_string()
}

/// How long a call may take before we give up. Short on purpose: these sit
/// between the player and the game, so a dead server has to fail fast and say
/// so rather than look like a freeze.
const TIMEOUT_SECS: u64 = 10;

// ---------------------------------------------------------------- types ---

/// The signed-in player, as Discord knows them. Shown throughout the launcher;
/// grants nothing (the website checks the session on every call).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub username: String,
    #[serde(default)]
    pub avatar: Option<String>,
    /// A website admin: the launcher shows its admin tools. The website checks
    /// the permissions again on every admin call, so this only draws buttons.
    #[serde(default)]
    pub admin: bool,
    /// "review", "manage", "comments", "bans" (sp-website lib/board.ts).
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct MeOk {
    profile: Profile,
}

/// Who the session belongs to now: name, picture and admin rights as the
/// website sees them today (they change without a new sign-in).
pub async fn me(session: &str) -> Result<Profile> {
    let url = format!("{}/api/launcher/me", site_url());
    let res = send(client()?.get(url).bearer_auth(session)).await?;
    match res.status().as_u16() {
        200 => Ok(res.json::<MeOk>().await.map_err(|_| LauncherError::Message(OOPS.into()))?.profile),
        401 => Err(LauncherError::SignedOut),
        _ => Err(LauncherError::Message(OOPS.into())),
    }
}

/// What the UI needs at start, from this PC alone (no network).
#[derive(Debug, Clone, Serialize, Default)]
pub struct AuthState {
    pub profile: Option<Profile>,
}

#[derive(Debug, Clone, Deserialize)]
struct TokenOk {
    token: String,
    profile: Profile,
}

#[derive(Debug, Clone, Deserialize)]
struct PassOk {
    pass: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TicketOk {
    #[serde(default)]
    token: String,
    #[serde(default)]
    expires_in: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ApiError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    until: Option<String>,
}

/// The region the player's matches are in ("any", or one of `regions`), and
/// the regions with servers now with how many each (`game_region`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GameRegion {
    pub region: String,
    #[serde(default)]
    pub regions: std::collections::BTreeMap<String, u32>,
}

/// The player's in-game name, and when they may change it next: an ISO time,
/// or None when they may change it now (`game_name`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GameName {
    pub name: String,
    #[serde(default)]
    pub next_change_at: Option<String>,
}

/// The backend's one-time login ticket, on its way to the game process.
#[derive(Debug, Clone)]
pub struct Ticket {
    pub token: String,
    pub expires_in: u64,
}

// --------------------------------------------------------------- errors ---

/// What a player sees when something failed that they cannot act on: never an
/// HTTP status or an error page.
pub const OOPS: &str = "Oops, something went wrong. Try again in a moment.";

/// Turns a backend error code into something a player can act on. The codes
/// are shared with the backend (routes/launcher.js); an unknown one is kept in
/// brackets for whoever helps them, a bare HTTP status is not.
pub fn explain(code: &str, until: Option<&str>) -> String {
    match code {
        "KEY_SUSPENDED" => match until {
            Some(t) if !t.is_empty() => format!("Your account is suspended until {t}. Ask a moderator in Discord."),
            _ => "Your account is suspended. Ask a moderator in Discord.".into(),
        },
        "KEY_REVOKED" | "USER_BLOCKED" => "Your account has been banned from playing.".into(),
        "NOT_ENABLED" => "The game server does not accept Discord sign-in yet. Try again later.".into(),
        "PASS_INVALID" | "PASS_EXPIRED" | "PASS_USED" => "The server refused the sign-in pass. Press Play again.".into(),
        "RATE_LIMITED" => "Too many attempts. Wait a minute and try again.".into(),
        "PC_BANNED" => "This PC is banned from playing SUPER PEOPLE. If you think this is a mistake, ask a moderator in Discord.".into(),
        // The codes are read once per launcher run (pcid.rs): a restart reads them again.
        "PC_ID_REQUIRED" => "The launcher couldn't identify this PC, which is needed to play. Restart the launcher and try again. If it keeps happening, ask in Discord.".into(),
        // Changing the in-game name (`game_name`).
        "NAME_LENGTH" => "A name has 2 to 16 characters.".into(),
        "NAME_CHARACTERS" => "Use letters, numbers, _ . and - only, without spaces.".into(),
        "NAME_FORBIDDEN" => "That name isn't allowed. Pick another one.".into(),
        "NAME_TAKEN" => "Another player already has that name.".into(),
        "NAME_SAME" => "That's already your name.".into(),
        "NAME_COOLDOWN" => "You can change your name once every 14 days.".into(),
        "NO_CHARACTER" => "You don't have a character yet. Press Play once to create it, then you can change its name here.".into(),
        // Picking the region (`game_region`).
        "REGION_UNAVAILABLE" => "That region has no servers right now. Pick another one.".into(),
        "" => OOPS.into(),
        other if other.starts_with("HTTP_") => OOPS.into(),
        other => format!("Oops, something went wrong ({other}). Try again in a moment."),
    }
}

// ----------------------------------------------------------------- PKCE ---

/// A PKCE pair (RFC 7636, S256): the verifier stays here, the challenge goes
/// in the sign-in address.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Result<Pkce> {
    let mut bytes = [0u8; 32];
    random::fill(&mut bytes)?;
    let verifier = base64url(&bytes);
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    Ok(Pkce { verifier, challenge })
}

/// Unpadded base64url: 32 random bytes make a 43-character verifier.
pub fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..chunk.len() + 1 {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

mod random {
    use crate::error::{LauncherError, Result};

    /// The system's cryptographic generator (BCryptGenRandom).
    #[cfg(windows)]
    pub fn fill(buf: &mut [u8]) -> Result<()> {
        use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        let status = unsafe {
            BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
        };
        if status == 0 {
            Ok(())
        } else {
            Err(LauncherError::Message(format!("Could not get random bytes (0x{status:08x}).")))
        }
    }

    #[cfg(not(windows))]
    pub fn fill(buf: &mut [u8]) -> Result<()> {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
        Ok(())
    }
}

// -------------------------------------------------------------- sign-in ---

/// Where the Discord window starts.
pub fn sign_in_url(challenge: &str) -> String {
    format!("{}/api/auth/launcher?challenge={challenge}", site_url())
}

/// The sign-in answers with a redirect on to Discord. Anything else -- the site
/// down, an older site without the launcher sign-in, sign-in switched off --
/// would show an error page in the Discord window, so it is checked before the
/// window opens and said on the welcome screen instead.
pub async fn check_sign_in(url: &str) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(LauncherError::Http)?;
    let res = send(client.get(url)).await?;
    let to = res.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).unwrap_or("");
    if res.status().is_redirection() && !to.contains("/launcher/connected") {
        Ok(())
    } else {
        Err(LauncherError::Message(OOPS.into()))
    }
}

/// What the Discord window's address says once the site is done with it:
/// `None` while it is anywhere else, the one-time code, or why there is none.
pub fn read_connected(url: &str) -> Option<std::result::Result<String, String>> {
    let rest = url.strip_prefix(&format!("{}/launcher/connected", site_url()))?;
    let query = match rest.as_bytes().first() {
        None => "",
        Some(b'?') => &rest[1..],
        Some(_) => return None,
    };
    let code = query.split('&').find_map(|pair| pair.strip_prefix("code="));
    Some(match code {
        Some(code) if !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => Ok(code.to_string()),
        _ => Err(SIGN_IN_FAILED.into()),
    })
}

/// Trades the one-time code (and the verifier behind its challenge) for the
/// session.
pub async fn exchange(code: &str, verifier: &str) -> Result<(String, Profile)> {
    let url = format!("{}/api/launcher/token", site_url());
    let res = send(client()?.post(url).json(&serde_json::json!({ "code": code, "verifier": verifier }))).await?;
    if !res.status().is_success() {
        return Err(LauncherError::Message(SIGN_IN_FAILED.into()));
    }
    let ok: TokenOk = res.json().await.map_err(|_| LauncherError::Message(SIGN_IN_FAILED.into()))?;
    Ok((ok.token, ok.profile))
}

const SIGN_IN_FAILED: &str = "Oops, the Discord sign-in did not go through. Try again.";

// ----------------------------------------------------------------- terms ---
// Play needs the Terms of Service and the Privacy Policy accepted, here in the
// launcher: once per Discord account, and again whenever they change (their
// date on the website is the version). The website keeps who accepted which
// version (sp-website app/api/launcher/terms); the text comes from there too,
// so it is the same as on the website's own pages.

/// One of the two documents, as the website's legal pages have it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermsDoc {
    pub title: String,
    pub url: String,
    pub intro: String,
    pub sections: Vec<TermsSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermsSection {
    pub title: String,
    pub body: String,
}

/// The current terms and whether this Discord account accepted them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terms {
    pub version: String,
    pub accepted: bool,
    pub docs: Vec<TermsDoc>,
}

const TERMS_UNAVAILABLE: &str = "Could not load the Terms of Service. Try again in a moment.";

pub async fn terms(session: &str) -> Result<Terms> {
    let url = format!("{}/api/launcher/terms", site_url());
    let res = send(client()?.get(url).bearer_auth(session)).await?;
    match res.status().as_u16() {
        200 => res.json::<Terms>().await.map_err(|_| LauncherError::Message(TERMS_UNAVAILABLE.into())),
        401 => Err(LauncherError::SignedOut),
        _ => Err(LauncherError::Message(TERMS_UNAVAILABLE.into())),
    }
}

/// Records that the player accepted `version`, the one they were shown.
/// TermsRequired: the terms changed meanwhile, and that version is not the
/// current one any more.
pub async fn accept_terms(session: &str, version: &str) -> Result<()> {
    let url = format!("{}/api/launcher/terms", site_url());
    let res = send(client()?.post(url).bearer_auth(session).json(&serde_json::json!({ "version": version }))).await?;
    match res.status().as_u16() {
        200 => Ok(()),
        401 => Err(LauncherError::SignedOut),
        409 => Err(LauncherError::TermsRequired),
        _ => Err(LauncherError::Message(OOPS.into())),
    }
}

// ------------------------------------------------------------------ Play ---

/// A game pass from the website for this session.
pub async fn game_pass(session: &str) -> Result<String> {
    let url = format!("{}/api/launcher/pass", site_url());
    let res = send(client()?.post(url).bearer_auth(session)).await?;
    match res.status().as_u16() {
        200 => Ok(res.json::<PassOk>().await.map_err(|_| LauncherError::Message(OOPS.into()))?.pass),
        401 => Err(LauncherError::SignedOut),
        // The website insists on the terms too (LAUNCHER_TERMS_REQUIRED), and
        // they changed since this launcher last asked.
        403 => match res.json::<ApiError>().await.unwrap_or_default().error.as_str() {
            "terms" => Err(LauncherError::TermsRequired),
            _ => Err(LauncherError::Message(OOPS.into())),
        },
        503 => Err(LauncherError::Message(explain("NOT_ENABLED", None))),
        _ => Err(LauncherError::Message(OOPS.into())),
    }
}

/// Hands the pass to the game backend, which answers with the ticket the game
/// logs in with (and remembers this launch for the game's login).
pub async fn discord_launch(pass: &str, device_id: &str, pc: &pcid::Codes) -> Result<Ticket> {
    let url = format!("{AUTH_BASE_URL}/session/discord");
    let res = send(client()?.post(url).json(&launch_body(pass, device_id, pc))).await?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if status.is_success() {
        let ok: TicketOk = serde_json::from_str(&text).map_err(|_| LauncherError::Message(OOPS.into()))?;
        if ok.token.is_empty() {
            return Err(LauncherError::Message(OOPS.into()));
        }
        return Ok(Ticket { token: ok.token, expires_in: ok.expires_in });
    }
    let err: ApiError = serde_json::from_str(&text).unwrap_or_default();
    let code = if err.error.is_empty() { format!("HTTP_{}", status.as_u16()) } else { err.error };
    Err(LauncherError::Message(explain(&code, err.until.as_deref())))
}

/// Reads the player's in-game name (`name` None) or changes it, with a fresh
/// game pass: the backend (routes/launcher.js POST /account/name) only renames
/// the character of the Discord account the website signed the pass for, once
/// every 14 days, and checks the name itself.
pub async fn game_name(session: &str, name: Option<&str>) -> Result<GameName> {
    let pass = game_pass(session).await?;
    let url = format!("{AUTH_BASE_URL}/account/name");
    let res = send(client()?.post(url).json(&name_body(&pass, name))).await?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|_| LauncherError::Message(OOPS.into()));
    }
    let err: ApiError = serde_json::from_str(&text).unwrap_or_default();
    // A backend from before the route: a 404 without a code of ours.
    if status.as_u16() == 404 && err.error.is_empty() {
        return Err(LauncherError::Message("The game server can't change names yet. Try again later.".into()));
    }
    let code = if err.error.is_empty() { format!("HTTP_{}", status.as_u16()) } else { err.error };
    Err(LauncherError::Message(explain(&code, err.until.as_deref())))
}

/// Reads the region the player's matches are in (`region` None) or picks one
/// ("any" or a region with servers), with a fresh game pass: the backend
/// (routes/launcher.js POST /account/region) keeps it for that Discord
/// account, and a party plays where its leader picked.
pub async fn game_region(session: &str, region: Option<&str>) -> Result<GameRegion> {
    let pass = game_pass(session).await?;
    let url = format!("{AUTH_BASE_URL}/account/region");
    let body = match region {
        Some(region) => serde_json::json!({ "pass": pass, "region": region }),
        None => serde_json::json!({ "pass": pass }),
    };
    let res = send(client()?.post(url).json(&body)).await?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if status.is_success() {
        return serde_json::from_str(&text).map_err(|_| LauncherError::Message(OOPS.into()));
    }
    let err: ApiError = serde_json::from_str(&text).unwrap_or_default();
    // A backend from before regions: no regions, so the picker stays hidden.
    if status.as_u16() == 404 && err.error.is_empty() {
        return Ok(GameRegion { region: "any".into(), regions: Default::default() });
    }
    let code = if err.error.is_empty() { format!("HTTP_{}", status.as_u16()) } else { err.error };
    Err(LauncherError::Message(explain(&code, err.until.as_deref())))
}

fn name_body(pass: &str, name: Option<&str>) -> serde_json::Value {
    match name {
        Some(name) => serde_json::json!({ "pass": pass, "name": name }),
        None => serde_json::json!({ "pass": pass }),
    }
}

/// What Play sends the backend: the pass, this install's id, and this PC's
/// one-way codes, `{"v": 1, "board", "disk", "windows"}`, each code left out
/// when it could not be read. A backend that does not know `pc` ignores it.
fn launch_body(pass: &str, device_id: &str, pc: &pcid::Codes) -> serde_json::Value {
    serde_json::json!({ "pass": pass, "device_id": device_id, "pc": pc })
}

// ------------------------------------------------------------------ logs ---
// The team's Discord log channels, through the website (sp-website
// lib/discord.ts): the launcher never holds a webhook. Best effort: nothing
// waits on them, and a failure is not the player's problem.

/// What the player did with the game (#launcher-logs): `{"action": …}` plus
/// numbers (files, bytes, seconds) or the reason a download failed.
pub fn report(session: String, mut event: serde_json::Value) {
    if let Some(fields) = event.as_object_mut() {
        fields.insert("version".into(), env!("CARGO_PKG_VERSION").into());
    }
    let launched = event.get("action").and_then(|a| a.as_str()) == Some("game.launched");
    tauri::async_runtime::spawn(async move {
        // The game start's log line shows the player's IP as the website saw
        // it. A PC with IPv6 reaches the site over it, so first a check-in
        // over IPv4 (sp-website app/api/launcher/ipv4): the line then shows
        // both. No IPv4 route, or an older website: nothing, and on we go.
        if launched {
            if let Ok(v4) = ipv4_client() {
                let url = format!("{}/api/launcher/ipv4", site_url());
                let _ = v4.post(url).bearer_auth(&session).timeout(std::time::Duration::from_secs(4)).send().await;
            }
        }
        let Ok(client) = client() else { return };
        let url = format!("{}/api/launcher/log", site_url());
        let _ = client.post(url).bearer_auth(session).json(&event).send().await;
    });
}

/// Looks names up to their IPv4 addresses only, so a client using it
/// connects over IPv4 or not at all.
struct Ipv4Only;

impl reqwest::dns::Resolve for Ipv4Only {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let found = tokio::task::spawn_blocking(move || {
                use std::net::ToSocketAddrs;
                (host.as_str(), 0).to_socket_addrs().map(|all| all.filter(|a| a.is_ipv4()).collect::<Vec<_>>())
            })
            .await??;
            if found.is_empty() {
                return Err("no IPv4 address".into());
            }
            Ok(Box::new(found.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn ipv4_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .dns_resolver(std::sync::Arc::new(Ipv4Only))
        .build()
        .map_err(LauncherError::Http)
}

/// The player disconnected the launcher (#discord-auth-logs). Waits a few
/// seconds at most: the sign-out itself is on this PC and happens anyway.
pub async fn signed_out(session: &str) {
    let Ok(client) = client() else { return };
    let url = format!("{}/api/launcher/signout", site_url());
    let _ = client.post(url).bearer_auth(session).timeout(std::time::Duration::from_secs(5)).send().await;
}

// ------------------------------------------------------------ http calls ---

pub fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(LauncherError::Http)
}

/// Sends, turning the failures a player actually hits into plain sentences.
pub async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    request.send().await.map_err(|e| {
        if e.is_timeout() {
            LauncherError::Message("The server did not respond. Try again in a moment.".into())
        } else if e.is_connect() {
            LauncherError::Message("Could not reach the server. Check your connection, or ask in Discord whether it is up.".into())
        } else {
            LauncherError::Http(e)
        }
    })
}

// ----------------------------------------------------- secrets at rest ---

/// Hex rather than base64 so there is no dependency for it, and so a config
/// file is obviously-opaque instead of looking like readable text.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(windows)]
mod secret {
    //! DPAPI, user scope: only this Windows account on this machine can
    //! decrypt the blob. Copying config.v1.json to another PC gets an attacker
    //! nothing, which is the point.
    //!
    //! Signatures checked against windows-sys 0.59:
    //!   CryptProtectData(*const BLOB, PCWSTR, *const BLOB, *const c_void,
    //!                    *const CRYPTPROTECT_PROMPTSTRUCT, u32, *mut BLOB) -> BOOL
    //!   CryptUnprotectData(*const BLOB, *mut PWSTR, *const BLOB, *const c_void,
    //!                      *const CRYPTPROTECT_PROMPTSTRUCT, u32, *mut BLOB) -> BOOL
    //!   LocalFree(HLOCAL) -> HLOCAL, HLOCAL = *mut c_void
    //! Windows frees the output buffer's memory, not us, so every success path
    //! copies it out and then LocalFree's it.
    use crate::error::{LauncherError, Result};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    fn input_blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        // The API does not write through pdatain; the cast is only needed
        // because the struct field is typed *mut.
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    /// Copies the result out and hands the buffer back to Windows.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        if out.pbData.is_null() {
            return Vec::new();
        }
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        LocalFree(out.pbData as *mut core::ffi::c_void);
        v
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = input_blob(plain);
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),      // szDataDescr
                std::ptr::null(),      // pOptionalEntropy
                std::ptr::null(),      // pvReserved
                std::ptr::null(),      // pPromptStruct
                0,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(LauncherError::Message("Windows refused to encrypt your sign-in.".into()));
        }
        Ok(unsafe { take(output) })
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
        let input = input_blob(sealed);
        let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),  // ppszDataDescr
                std::ptr::null(),      // pOptionalEntropy
                std::ptr::null(),      // pvReserved
                std::ptr::null(),      // pPromptStruct
                0,
                &mut output,
            )
        };
        if ok == 0 {
            // Wrong user, wrong machine, or a corrupted blob. All of them mean
            // the same thing to the player: sign in again.
            return Err(LauncherError::Message(
                "The saved sign-in could not be read. Connect again.".into(),
            ));
        }
        Ok(unsafe { take(output) })
    }
}

#[cfg(not(windows))]
mod secret {
    //! The launcher ships for Windows only; this exists so the crate builds and
    //! its tests run on a developer machine. It is NOT encryption and does not
    //! pretend to be — it is a passthrough, and the comment is here so nobody
    //! ever mistakes a non-Windows build for something safe to hand to players.
    use crate::error::Result;
    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        Ok(plain.to_vec())
    }
    pub fn unprotect(blob: &[u8]) -> Result<Vec<u8>> {
        Ok(blob.to_vec())
    }
}

/// Encrypts a secret (the Discord session) for storage in the config file.
pub fn seal(plain: &str) -> Result<String> {
    Ok(to_hex(&secret::protect(plain.as_bytes())?))
}

/// Reverses `seal`. Any failure means "sign in again": a session that cannot
/// be decrypted cannot be used.
pub fn unseal(stored: &str) -> Result<String> {
    let bytes = from_hex(stored)
        .ok_or_else(|| LauncherError::Message("The saved sign-in is damaged. Connect again.".into()))?;
    let plain = secret::unprotect(&bytes)?;
    String::from_utf8(plain)
        .map_err(|_| LauncherError::Message("The saved sign-in is damaged. Connect again.".into()))
}

// -------------------------------------------------------------- device id ---

/// Identifies this installation to the backend (it records it per account).
///
/// Deliberately not cryptographic: it is a label, not a secret, and the
/// backend treats it as one. Uniqueness is all that is needed, so it is built
/// from the clock plus the address-space randomness the standard library
/// already has, which avoids adding a random-number dependency for a value
/// that is generated exactly once per install.
pub fn new_device_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let mut h = RandomState::new().build_hasher();
    h.write_u128(nanos);
    let a = h.finish();
    let mut h2 = RandomState::new().build_hasher();
    h2.write_u64(a);
    h2.write_u128(nanos);
    let b = h2.finish();

    format!("{a:016x}{b:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(base64url(&Sha256::digest(verifier.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn a_fresh_pkce_pair_is_the_right_shape_and_new_each_time() {
        let a = pkce().unwrap();
        let b = pkce().unwrap();
        assert_eq!(a.verifier.len(), 43);
        assert_eq!(a.challenge.len(), 43);
        assert_ne!(a.verifier, b.verifier);
        assert_eq!(a.challenge, base64url(&Sha256::digest(a.verifier.as_bytes())));
    }

    #[test]
    fn base64url_has_no_padding() {
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn the_connected_page_gives_the_code_and_nothing_else_does() {
        let site = site_url();
        assert_eq!(read_connected(&format!("{site}/launcher/connected?code=3f2a-9c")), Some(Ok("3f2a-9c".into())));
        assert!(matches!(read_connected(&format!("{site}/launcher/connected?error=failed")), Some(Err(_))));
        assert!(matches!(read_connected(&format!("{site}/launcher/connected")), Some(Err(_))));
        assert_eq!(read_connected("https://discord.com/oauth2/authorize?x=1"), None);
        assert_eq!(read_connected(&format!("{site}/api/auth/discord/callback?code=x")), None);
        assert_eq!(read_connected(&format!("{site}/launcher/connectedX?code=x")), None);
        // Another site with the same path is not ours.
        assert_eq!(read_connected("https://evil.example/launcher/connected?code=abc"), None);
        assert!(matches!(read_connected(&format!("{site}/launcher/connected?code=<script>")), Some(Err(_))));
    }

    #[test]
    fn hex_round_trips() {
        let data = b"\x00\x01\xfe\xff hello";
        assert_eq!(from_hex(&to_hex(data)).unwrap(), data);
        assert!(from_hex("abc").is_none());      // odd length
        assert!(from_hex("zz").is_none());       // not hex
    }

    #[test]
    fn a_sealed_session_comes_back_out() {
        let sealed = seal("session-token.signature").unwrap();
        assert_ne!(sealed, "session-token.signature", "must not be stored in the clear");
        assert_eq!(unseal(&sealed).unwrap(), "session-token.signature");
    }

    #[test]
    fn damaged_storage_asks_to_connect_again_instead_of_panicking() {
        assert!(unseal("not hex at all").is_err());
        assert!(unseal("abc").is_err());
    }

    #[test]
    fn device_ids_are_unique_and_the_right_shape() {
        let a = new_device_id();
        let b = new_device_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn error_codes_become_sentences_a_player_can_act_on() {
        assert!(explain("KEY_SUSPENDED", Some("2026-01-01T00:00:00Z")).contains("2026-01-01"));
        assert!(explain("KEY_SUSPENDED", None).contains("suspended"));
        assert!(explain("KEY_REVOKED", None).contains("banned"));
        assert!(explain("PASS_USED", None).contains("Play again"));
        assert!(explain("PC_BANNED", None).contains("This PC is banned"));
        assert!(explain("PC_ID_REQUIRED", None).contains("Restart the launcher"));
        // An unfamiliar code must still be visible, not swallowed.
        assert!(explain("SOME_NEW_CODE", None).contains("SOME_NEW_CODE"));
        // A bare HTTP status (an error page, a missing route) is never shown.
        assert_eq!(explain("HTTP_404", None), OOPS);
        assert!(!explain("HTTP_502", None).contains("502"));
    }

    #[test]
    fn renaming_reads_without_a_name_and_explains_every_refusal() {
        assert_eq!(name_body("p", None), serde_json::json!({ "pass": "p" }));
        assert_eq!(name_body("p", Some("Neo")), serde_json::json!({ "pass": "p", "name": "Neo" }));
        for code in ["NAME_LENGTH", "NAME_CHARACTERS", "NAME_FORBIDDEN", "NAME_TAKEN", "NAME_SAME", "NAME_COOLDOWN", "NO_CHARACTER"] {
            let text = explain(code, None);
            assert!(!text.contains(code) && text != OOPS, "{code}: {text}");
        }
        let read: GameName = serde_json::from_str(r#"{"ok":true,"name":"Neo","next_change_at":null}"#).unwrap();
        assert_eq!(read, GameName { name: "Neo".into(), next_change_at: None });
    }

    #[test]
    fn the_region_answer_reads_and_a_refusal_is_a_sentence() {
        let got: GameRegion = serde_json::from_str(r#"{"ok":true,"region":"asia","regions":{"europe":2,"asia":1}}"#).unwrap();
        assert_eq!(got.region, "asia");
        assert_eq!(got.regions.get("europe"), Some(&2));
        assert!(explain("REGION_UNAVAILABLE", None).contains("no servers"));
    }

    #[test]
    fn play_sends_the_pc_codes_it_has_and_always_v() {
        let none = pcid::Codes::default();
        assert_eq!(
            launch_body("p", "d", &none),
            serde_json::json!({ "pass": "p", "device_id": "d", "pc": { "v": 1 } })
        );
        let two = pcid::Codes { board: Some("b".into()), windows: Some("w".into()), ..pcid::Codes::default() };
        assert_eq!(
            launch_body("p", "d", &two),
            serde_json::json!({ "pass": "p", "device_id": "d", "pc": { "v": 1, "board": "b", "windows": "w" } })
        );
    }

    #[tokio::test]
    async fn the_check_in_goes_over_ipv4() {
        use reqwest::dns::Resolve;
        let found: Vec<_> = Ipv4Only.resolve("localhost".parse().unwrap()).await.expect("localhost").collect();
        assert!(!found.is_empty() && found.iter().all(|a| a.is_ipv4()), "{found:?}");

        // A server on IPv4 sees the client come from an IPv4 address.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut sock, from) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            from
        });
        let res = ipv4_client().unwrap().get(format!("http://localhost:{port}/")).send().await.expect("sent");
        assert_eq!(res.status().as_u16(), 204);
        assert!(peer.await.unwrap().is_ipv4());
    }

    #[test]
    fn terms_read_as_the_website_sends_them() {
        // sp-website app/api/launcher/terms, GET: acceptedAt is not needed here.
        let json = r#"{"version":"2026-09-30","docs":[
            {"title":"Terms of Service","url":"https://superpeople.dev/terms","intro":"These terms apply.",
             "sections":[{"title":"A fan project","body":"Non-commercial."}]},
            {"title":"Privacy Policy","url":"https://superpeople.dev/privacy","intro":"As little as we can.","sections":[]}
        ],"accepted":false,"acceptedAt":null}"#;
        let terms: Terms = serde_json::from_str(json).expect("terms");
        assert_eq!(terms.version, "2026-09-30");
        assert!(!terms.accepted);
        assert_eq!(terms.docs.len(), 2);
        assert_eq!(terms.docs[0].sections[0].title, "A fan project");
    }
}
