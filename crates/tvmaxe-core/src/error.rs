use crate::i18n::{fill, tr};
use std::fmt;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum Error {
    /// Eroare SQLite (baza proprie sau o listă `.db` importată).
    Db(rusqlite::Error),
    /// Fișier local inaccesibil.
    Io(std::io::Error),
    /// Descărcare eșuată.
    Http(String),
    /// Conținutul nu e o listă recunoscută.
    UnknownFormat,
    /// Lista e validă, dar nu conține niciun canal.
    Empty,
    /// Listă coruptă (detaliu în mesaj).
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Db(e) => f.write_str(&fill(tr("database error: %1"), &[e])),
            Error::Io(e) => write!(f, "{e}"),
            Error::Http(e) => f.write_str(&fill(tr("download failed: %1"), &[e])),
            Error::UnknownFormat => f.write_str(tr(
                "unknown list format (TV-Maxe .db and M3U are supported)",
            )),
            Error::Empty => f.write_str(tr("the list contains no channels")),
            Error::Invalid(e) => f.write_str(&fill(tr("invalid list: %1"), &[e])),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
