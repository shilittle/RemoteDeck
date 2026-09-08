use serde::ser::{Serialize, Serializer};
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("state error: {0}")]
    State(String),
    #[error("invalid input: {0}")]
    Validation(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("required executable is unavailable: {0}")]
    MissingExecutable(String),
    #[error("process failed: {0}")]
    Process(String),
    #[error("process cleanup failed: {0}")]
    ProcessCleanup(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error("operation timed out: {0}")]
    Timeout(String),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("archive error: {0}")]
    Archive(#[from] zip::result::ZipError),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
