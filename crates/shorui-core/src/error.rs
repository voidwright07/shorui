use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

/// Every message here is shown to the user as is, so each one says what happened
/// and, where there is one, what to do next.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Could not read {}: {source}", path.display())]
    Read { path: PathBuf, source: std::io::Error },

    #[error("Could not write {}: {source}", path.display())]
    Write { path: PathBuf, source: std::io::Error },

    #[error("This file is password protected. Unlock it first, or enter its password.")]
    PasswordRequired,

    #[error("That password is not correct.")]
    WrongPassword,

    /// The file could not be parsed. The message names the damaged part.
    #[error("{0}")]
    Damaged(String),

    /// An option or input the user gave cannot be used.
    #[error("{0}")]
    Invalid(String),

    #[error("{0}")]
    Unsupported(String),

    /// A helper program this tool relies on is not installed.
    #[error("{tool} was not found on this machine. {hint}")]
    MissingHelper { tool: String, hint: String },

    /// A helper program ran and failed.
    #[error("{0}")]
    External(String),

    #[error("Cancelled.")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn read(path: &Path, source: std::io::Error) -> Self {
        Error::Read { path: path.to_path_buf(), source }
    }
    pub fn write(path: &Path, source: std::io::Error) -> Self {
        Error::Write { path: path.to_path_buf(), source }
    }
    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
    pub fn damaged(msg: impl Into<String>) -> Self {
        Error::Damaged(msg.into())
    }
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}

impl From<lopdf::Error> for Error {
    fn from(e: lopdf::Error) -> Self {
        use lopdf::Error as L;
        match e {
            L::InvalidPassword => Error::WrongPassword,
            L::Decryption(lopdf::encryption::DecryptionError::IncorrectPassword) => Error::WrongPassword,
            L::IO(io) => Error::Other(format!("File error: {io}")),
            L::Xref(_) | L::MissingXrefEntry | L::InvalidOffset(_) => Error::Damaged(
                "The page index in this file is damaged, so it could not be read. Repair can usually rebuild it.".into(),
            ),
            L::Parse(_) | L::IndirectObject { .. } | L::InvalidStream(_) | L::InvalidObjectStream(_) => Error::Damaged(
                "Part of this file is damaged and could not be read. Repair can usually rebuild it.".into(),
            ),
            other => Error::Other(format!("PDF error: {other}")),
        }
    }
}

impl From<image::ImageError> for Error {
    fn from(e: image::ImageError) -> Self {
        Error::Other(format!("Image error: {e}"))
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Other(format!("File error: {e}"))
    }
}
