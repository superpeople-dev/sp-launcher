//! Locating and starting the game.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{LauncherError, Result};

/// Relative to the install directory.
pub const GAME_EXE: &str = "BravoHotelClient.exe";

/// Folders that should sit alongside `GAME_EXE` in the install directory.
/// There is no manifest to verify file-by-file against yet, so this — plus
/// the executable itself — is the whole "is this actually installed" check:
/// good enough to recognize a folder someone already has the game in
/// (copied from Steam, a previous install, wherever) without downloading
/// anything.
const REQUIRED_DIRS: &[&str] = &["BravoHotelGame", "Engine"];

#[derive(Debug, Clone, Serialize)]
pub struct InstallState {
    pub installed: bool,
    pub exe_path: Option<String>,
}

pub fn exe_path(install_dir: &Path) -> PathBuf {
    install_dir.join(GAME_EXE)
}

/// The real game, started directly. `BravoHotelClient.exe` in the root is only
/// Unreal's bootstrapper (it checks prerequisites, then starts this exe with
/// the same arguments) and its manifest demands admin rights, which a
/// non-elevated launcher cannot satisfy (os error 740). The shipping exe runs
/// as the normal user. Falls back to the bootstrapper if it is missing.
pub const SHIPPING_EXE: &str = r"BravoHotelGame\Binaries\Win64\BravoHotelClient-Win64-Shipping.exe";

pub fn launch_exe_path(install_dir: &Path) -> PathBuf {
    let shipping = install_dir.join(SHIPPING_EXE);
    if shipping.is_file() {
        shipping
    } else {
        exe_path(install_dir)
    }
}

pub fn detect(install_dir: &str) -> InstallState {
    if install_dir.is_empty() {
        return InstallState { installed: false, exe_path: None };
    }
    let root = Path::new(install_dir);
    let exe = exe_path(root);
    let complete = exe.is_file() && REQUIRED_DIRS.iter().all(|dir| root.join(dir).is_dir());
    if complete {
        InstallState { installed: true, exe_path: Some(exe.to_string_lossy().into_owned()) }
    } else {
        InstallState { installed: false, exe_path: None }
    }
}

/// Splits the user's arguments box into argv entries.
///
/// Newlines and spaces both separate, and double quotes group — so a path with
/// a space can be passed as `-Foo="C:\Program Files\x"`. Passing argv directly
/// (never a command string) is what keeps this free of shell-injection.
pub fn parse_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;

    for ch in raw.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                cur.push(ch);
            }
            c if c.is_whitespace() && !in_quotes => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The arguments the game does not work without, kept here rather than in the
/// player's settings.
///
/// `-IgnoreCatalogue` skips a catalogue fetch that cannot succeed.
/// `-ApiPhase` has to match `apiPhase` in the backend config.
/// `-ServicePlatform` names an online-subsystem module: the client appends the
/// value to "OnlineSubsystem" and loads that. An EMPTY value resolves to
/// "Internal", which is the client's own account path -- the login screen with
/// a username box. That is the path we want: the backend recognises the
/// launcher's key and logs the player in on it without anyone typing anything,
/// and no Steam client or game ownership is involved.
///
/// (It used to say Steam here, on the belief that Internal was not in the build
/// and would hang on the loading screen. That was wrong -- a real run reached
/// the Internal login screen and completed.)
///
/// These are invisible to the player on purpose: they are not preferences, and
/// a launcher that lets someone delete them is a launcher that lets someone
/// break their own install.
pub const BASE_ARGS: &[&str] = &[
    "-IgnoreCatalogue",
    "-ApiPhase=\"dev2s\"",
    // Empty on purpose. "Steam" sends the client down the Steam login path,
    // which needs Steam running and an account that owns the game. An EMPTY
    // value resolves to the client's `Internal` subsystem, which is its own
    // account path -- and that is the one the backend logs players in on
    // automatically from the launcher's key. Do not "fix" this to Steam.
    "-ServicePlatform=",
];

/// Builds the full argv: the server address first (if one was entered in the
/// connect prompt), then the required arguments, then whatever the player added
/// on top. Split out from `launch` so it can be tested without spawning.
fn full_args(server: Option<&str>, user_args: &str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(server) = server {
        if !server.is_empty() {
            args.push(server.to_string());
        }
    }
    // A required argument is skipped when the player already supplied the same
    // one. Unreal's parser takes the FIRST occurrence, so appending ours first
    // and theirs second would silently ignore whatever they typed -- which is
    // the opposite of what an override is for.
    let user = parse_args(user_args);
    for base in BASE_ARGS {
        if EMPTY_VALUE_LAST.contains(base) {
            continue;                       // emitted after the player's args, see below
        }
        if !user.iter().any(|u| same_switch(u, base)) {
            args.push((*base).to_string());
        }
    }
    args.extend(user.clone());

    // An argument whose value is EMPTY has to be the last thing on the command
    // line. Unreal's parser reads the text after `=` up to whitespace, and with
    // nothing there it takes the NEXT TOKEN instead. A real run proved it: with
    // `-ServicePlatform= -loginauto`, the game tried to load an online
    // subsystem module literally called "-loginauto" and logged
    // `D03001 Failed to load module`. Anything the player types would be
    // swallowed the same way, so these go at the very end where nothing follows.
    for base in BASE_ARGS {
        if !EMPTY_VALUE_LAST.contains(base) {
            continue;
        }
        if !user.iter().any(|u| same_switch(u, base)) {
            args.push((*base).to_string());
        }
    }
    args
}

/// Required arguments whose value is empty, which therefore must come last.
const EMPTY_VALUE_LAST: &[&str] = &["-ServicePlatform="];

/// Do two tokens set the same switch? `-ApiPhase="dev2s"` and `-ApiPhase=x` do;
/// `-ServicePlatform=Steam` and `-ServicePlatform=` do, which is the case that
/// matters for testing a different platform.
fn same_switch(a: &str, b: &str) -> bool {
    let key = |s: &str| -> String {
        let s = s.trim_start_matches('-');
        match s.find('=') {
            Some(i) => s[..i].to_ascii_lowercase(),
            None => s.to_ascii_lowercase(),
        }
    };
    key(a) == key(b)
}

pub struct LaunchSpec<'a> {
    pub install_dir: &'a str,
    /// Extra environment variables for the game process only.
    ///
    /// This is how the one-time login ticket reaches the game, and the choice
    /// of environment over a command-line argument is deliberate: on Windows
    /// any process can read another's command line, while its environment
    /// block cannot be read without debug privileges. A ticket on the command
    /// line would be visible in Task Manager.
    pub env: &'a [(String, String)],
    /// `ip:port` typed into the connect prompt, or `None`/empty for "Play
    /// without joining server" — asked fresh on every launch rather than
    /// stored, since who to connect to can change launch to launch.
    pub server: Option<&'a str>,
    pub user_args: &'a str,
}

pub fn launch(spec: LaunchSpec<'_>) -> Result<std::process::Child> {
    if spec.install_dir.is_empty() {
        return Err(LauncherError::Message("no install directory set".into()));
    }
    let exe = launch_exe_path(Path::new(spec.install_dir));
    if !exe.is_file() {
        return Err(LauncherError::Message(format!(
            "game executable not found at {}",
            exe.display()
        )));
    }

    // The backend is reached through hosts redirection, not through arguments,
    // so the rest of the command line is just the (optional) server address
    // plus whatever the user configured.
    let args: Vec<String> = full_args(spec.server, spec.user_args);

    let working_dir = exe.parent().ok_or_else(|| {
        LauncherError::Message("game executable has no parent directory".into())
    })?;

    // Returning the Child, not just the pid: the caller waits on it so the
    // hosts entries come out again the moment the game exits.
    build_command(&exe, &args, working_dir, spec.env)
        .spawn()
        .map_err(|e| LauncherError::Message(format!("could not start the game: {e}")))
}

/// Assembles the process without spawning it, so what ends up in argv and what
/// ends up in the environment can both be asserted in a test. That split is
/// security-relevant -- the login ticket must be in the environment and must
/// never appear on the command line -- so it is worth being able to prove.
fn build_command(
    exe: &Path,
    args: &[String],
    working_dir: &Path,
    env: &[(String, String)],
) -> Command {
    let mut cmd = Command::new(exe);
    cmd.current_dir(working_dir);
    for arg in args {
        #[cfg(windows)]
        if same_switch(arg, "-ExecCmds=") {
            // Unreal parses the raw Windows command line. Keep the quotes
            // around the value; Command::arg would escape and quote them again.
            use std::os::windows::process::CommandExt;
            cmd.raw_arg(arg);
            continue;
        }
        cmd.arg(arg);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::{build_command, detect, full_args, parse_args};
    use std::ffi::OsStr;
    use std::fs;
    use std::path::Path;

    use super::BASE_ARGS;

    fn base() -> Vec<String> {
        BASE_ARGS.iter().map(|a| (*a).to_string()).collect()
    }

    #[test]
    fn the_required_arguments_are_always_there() {
        // Even with nothing configured and nowhere to connect.
        assert_eq!(full_args(None, ""), base());
    }

    #[test]
    fn server_address_comes_first_then_the_required_arguments() {
        let got = full_args(Some("203.0.113.10:27015"), "");
        assert_eq!(got[0], "203.0.113.10:27015");
        // Same set, but -ServicePlatform= is moved to the end (see below).
        let mut want: Vec<String> = base().into_iter().filter(|a| a != "-ServicePlatform=").collect();
        want.push("-ServicePlatform=".to_string());
        assert_eq!(got[1..], want[..]);
    }

    #[test]
    fn an_empty_valued_argument_is_last_so_it_cannot_swallow_the_next_one() {
        // The bug this exists to prevent: Unreal read `-loginauto` as the VALUE
        // of `-ServicePlatform=` and tried to load it as a module.
        let got = full_args(None, "-loginauto -windowed");
        assert_eq!(got.last().unwrap(), "-ServicePlatform=", "{got:?}");
        assert!(got.contains(&"-loginauto".to_string()));
        assert!(got.contains(&"-windowed".to_string()));
    }

    #[test]
    fn a_player_override_replaces_the_required_argument() {
        // Unreal takes the first occurrence, so ours must not be emitted at all.
        let got = full_args(None, "-ServicePlatform=Steam");
        assert_eq!(got.iter().filter(|a| a.starts_with("-ServicePlatform")).count(), 1, "{got:?}");
        assert!(got.contains(&"-ServicePlatform=Steam".to_string()), "{got:?}");
        assert!(!got.contains(&"-ServicePlatform=".to_string()), "{got:?}");
        // The others are untouched.
        assert!(got.contains(&"-IgnoreCatalogue".to_string()));
    }

    #[test]
    fn an_override_is_matched_regardless_of_value_or_case() {
        let got = full_args(None, "-serviceplatform=Null");
        assert_eq!(got.iter().filter(|a| a.to_lowercase().starts_with("-serviceplatform")).count(), 1, "{got:?}");
    }

    #[test]
    fn the_players_own_arguments_go_after_the_required_ones() {
        let got = full_args(None, "-windowed -ResX=1280");
        let mut want: Vec<String> = base().into_iter().filter(|a| a != "-ServicePlatform=").collect();
        want.extend(["-windowed".to_string(), "-ResX=1280".to_string()]);
        want.push("-ServicePlatform=".to_string());
        assert_eq!(got, want);
    }

    #[test]
    fn an_empty_server_address_is_treated_the_same_as_none() {
        assert_eq!(full_args(Some(""), "-windowed"), full_args(None, "-windowed"));
    }

    #[test]
    fn the_service_platform_is_empty_because_that_is_the_account_login_path() {
        // An empty value is NOT the same as omitting the switch: omitting it
        // lets the client pick its configured default, which is Steam.
        let got = full_args(None, "");
        assert!(got.iter().any(|a| a == "-ServicePlatform="), "{got:?}");
        assert!(!got.iter().any(|a| a == "-ServicePlatform=Steam"), "{got:?}");
    }

    #[test]
    fn empty_install_dir_is_never_installed() {
        assert!(!detect("").installed);
    }

    #[test]
    fn missing_folder_is_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        // Doesn't exist at all.
        let missing = dir.path().join("nope");
        assert!(!detect(missing.to_str().unwrap()).installed);
    }

    #[test]
    fn needs_all_three_to_count_as_installed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Nothing there yet.
        assert!(!detect(root.to_str().unwrap()).installed);

        // Just the exe: still not installed.
        fs::write(root.join("BravoHotelClient.exe"), b"").unwrap();
        assert!(!detect(root.to_str().unwrap()).installed);

        // Exe plus one of the two folders: still not installed.
        fs::create_dir(root.join("BravoHotelGame")).unwrap();
        assert!(!detect(root.to_str().unwrap()).installed);

        // All three present: installed.
        fs::create_dir(root.join("Engine")).unwrap();
        let state = detect(root.to_str().unwrap());
        assert!(state.installed);
        assert!(state.exe_path.unwrap().ends_with("BravoHotelClient.exe"));
    }

    #[test]
    fn the_login_ticket_goes_in_the_environment_and_never_in_argv() {
        // On Windows any process can read another's command line; the
        // environment block cannot be read without debug privileges. A ticket
        // in argv would be visible in Task Manager, so this must stay true.
        let args = full_args(Some("1.2.3.4:7777"), "-IgnoreCatalogue");
        let env = vec![("SP_AUTH_TICKET".to_string(), "secret-token".to_string())];
        let cmd = build_command(Path::new("game.exe"), &args, Path::new("."), &env);

        let argv: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(
            !argv.iter().any(|a| a.contains("secret-token")),
            "the ticket must never reach the command line: {argv:?}"
        );

        let found = cmd
            .get_envs()
            .find(|(k, _)| *k == OsStr::new("SP_AUTH_TICKET"))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned());
        assert_eq!(found.as_deref(), Some("secret-token"));
    }

    #[test]
    fn no_env_means_nothing_extra_is_set() {
        let cmd = build_command(Path::new("game.exe"), &[], Path::new("."), &[]);
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn the_server_address_still_leads_the_command_line() {
        let args = full_args(Some("1.2.3.4:7777"), "-a -b");
        let env = vec![("SP_AUTH_TICKET".to_string(), "t".to_string())];
        let cmd = build_command(Path::new("game.exe"), &args, Path::new("."), &env);
        let argv: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();

        let mut want = vec!["1.2.3.4:7777".to_string()];
        want.extend(base().into_iter().filter(|a| a != "-ServicePlatform="));
        want.extend(["-a".to_string(), "-b".to_string()]);
        want.push("-ServicePlatform=".to_string());   // empty value, so it goes last
        assert_eq!(argv, want);
    }

    #[test]
    fn splits_on_spaces_and_newlines() {
        let got = parse_args("-windowed -ResX=1920\n-nosteam");
        assert_eq!(got, vec!["-windowed", "-ResX=1920", "-nosteam"]);
    }

    #[test]
    fn keeps_the_required_arguments_intact_when_split() {
        // UE strips the quotes itself; the token must reach it whole.
        let got = parse_args("-IgnoreCatalogue -ApiPhase=\"dev2s\"");
        assert_eq!(got, vec!["-IgnoreCatalogue", "-ApiPhase=\"dev2s\""]);
    }

    #[test]
    fn keeps_quoted_spaces_together() {
        let got = parse_args(r#"-Path="C:\Program Files\x" -y"#);
        assert_eq!(got, vec![r#"-Path="C:\Program Files\x""#, "-y"]);
    }

    #[cfg(windows)]
    #[test]
    fn captures_startup_command_line() {
        let Some(path) = std::env::var_os("SP_TEST_COMMAND_LINE_OUTPUT") else { return; };
        #[link(name = "kernel32")]
        extern "system" { fn GetCommandLineW() -> *const u16; }
        let text = unsafe {
            let raw = GetCommandLineW();
            let mut len = 0;
            while len < 32768 && *raw.add(len) != 0 { len += 1; }
            String::from_utf16_lossy(std::slice::from_raw_parts(raw, len))
        };
        fs::write(path, text).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn startup_command_quotes_reach_windows_exactly_once() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("command-line.txt");
        let args = vec![
            "--exact".to_string(),
            "game::tests::captures_startup_command_line".to_string(),
            "--nocapture".to_string(),
            "--".to_string(),
            "-ExecCmds=\"PakFile.SearchRecentlyFoundPaks 0,stat fps\"".to_string(),
        ];
        let env = vec![("SP_TEST_COMMAND_LINE_OUTPUT".to_string(), output.to_string_lossy().into_owned())];
        let status = build_command(&std::env::current_exe().unwrap(), &args, temp.path(), &env).status().unwrap();
        assert!(status.success());
        let raw = fs::read_to_string(output).unwrap();
        assert!(raw.contains("-ExecCmds=\"PakFile.SearchRecentlyFoundPaks 0,stat fps\""), "{raw}");
        assert!(!raw.contains("-ExecCmds=\"\""), "{raw}");
        assert!(!raw.contains(r#"\"PakFile.SearchRecentlyFoundPaks"#), "{raw}");
    }

    #[test]
    fn ignores_blank_runs() {
        assert_eq!(parse_args("  \n\n  -a   \n  -b  "), vec!["-a", "-b"]);
    }
}
