//! Budget4 core: the YNAB 4 budget format, sync engine, budget math and editing.
//!
//! See docs/ynab4-format.md for how the format works and what is verified against real data.

pub mod budget;
pub mod create;
pub mod dates;
pub mod editor;
pub mod importing;
pub mod json;
pub mod package;
pub mod schema;
pub mod serialize;
pub mod store;
pub mod vv;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Msg(String),
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
}

impl Error {
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io { path: path.display().to_string(), source }
    }
}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error::Msg(s)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
