use std::fmt;

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Db(rusqlite::Error),
    Json(serde_json::Error),
    #[allow(dead_code)]
    Clipboard(String),
    Daemon(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e)        => write!(f, "I/O: {e}"),
            Self::Db(e)        => write!(f, "database: {e}"),
            Self::Json(e)      => write!(f, "JSON: {e}"),
            Self::Clipboard(s) => write!(f, "clipboard: {s}"),
            Self::Daemon(s)    => write!(f, "daemon: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self { Self::Db(e) }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self { Self::Json(e) }
}

pub type Result<T> = std::result::Result<T, Error>;
