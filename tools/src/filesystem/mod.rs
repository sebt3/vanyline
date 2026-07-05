use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FilesystemError {
    #[error("path not allowed: {0}")]
    PathNotAllowed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::str::Utf8Error),
}

pub type Result<T> = std::result::Result<T, FilesystemError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadDirectoryOptions {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileOptions {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileOptions {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFileOptions {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDirectoryOptions {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDirectoryOptions {
    pub path: String,
}
