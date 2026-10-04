//! One error type for every command, serialised to the frontend as a string.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum LauncherError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("network: {0}")]
    Http(#[from] reqwest::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("{0}")]
    Message(String),

    /// The website no longer accepts the saved Discord sign-in (expired, or
    /// signed out elsewhere). lib.rs forgets it and shows the welcome screen.
    #[error("Your Discord sign-in has expired. Connect again to continue.")]
    SignedOut,

    /// Play needs the current Terms of Service accepted first (auth.rs
    /// terms). lib.rs tells the UI, which opens them.
    #[error("Accept the Terms of Service to play.")]
    TermsRequired,

    /// The website or the game backend requires a newer launcher (auth.rs
    /// VERSION). lib.rs tells the UI, which installs the update.
    #[error("This launcher is out of date. It is updating now; if it does not, close it and open it again.")]
    UpdateRequired,
    /// Windows' Smart App Control would refuse one of the game's community
    /// DLLs (smart_app_control.rs). lib.rs tells the UI, which explains it.
    #[error("Windows Smart App Control is blocking the game. The launcher shows what to do.")]
    SmartAppControl,
}

impl Serialize for LauncherError {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl From<String> for LauncherError {
    fn from(s: String) -> Self {
        LauncherError::Message(s)
    }
}

impl From<&str> for LauncherError {
    fn from(s: &str) -> Self {
        LauncherError::Message(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, LauncherError>;
