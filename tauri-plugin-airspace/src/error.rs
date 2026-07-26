use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("this plugin only does anything on Windows")]
    UnsupportedPlatform,

    #[error("no window with label `{0}`")]
    NoSuchWindow(String),

    #[error("window `{0}` has no native handle yet")]
    NoHandle(String),

    #[error("window `{0}` already has a native host; destroy it first")]
    HostExists(String),

    #[error("window `{0}` has no native host")]
    NoHost(String),

    #[error("CreateWindowEx failed for the native host (os error {0})")]
    CreateFailed(u32),

    /// The main thread didn't run our closure in time. Usually means the caller
    /// was already on the main thread, or the event loop is blocked. See the
    /// deadlock notes in the crate docs.
    #[error("timed out waiting for the main thread (are you calling this from the main thread, or from a sync command that is blocking the event loop?)")]
    MainThreadTimeout,

    #[error(transparent)]
    Tauri(#[from] tauri::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Serialize for Error {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
