//! Run deployment lifecycle tests without compiling the Tauri application.
mod error {
    #[derive(Debug)]
    pub enum LauncherError { Io(std::io::Error), Message(String) }
    impl From<std::io::Error> for LauncherError { fn from(e:std::io::Error)->Self { Self::Io(e) } }
    impl std::fmt::Display for LauncherError {
        fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
            match self { Self::Io(e)=>write!(f,"{e}"),Self::Message(s)=>write!(f,"{s}") }
        }
    }
    pub type Result<T> = std::result::Result<T,LauncherError>;
}
#[path = "../../src-tauri/src/client_fixes_deployment.rs"]
mod deployment;

#[path = "../../src-tauri/src/engine_ini.rs"]
mod engine_ini;
