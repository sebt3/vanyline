use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadFileOptions {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WriteFileOptions {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteFileOptions {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateDirectoryOptions {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListDirectoryOptions {
    pub path: String,
}

#[derive(Debug, Error)]
pub enum FilesystemError {
    #[error("path not allowed: {0}")]
    PathNotAllowed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn read_file(opts: ReadFileOptions) -> BoxedFuture<Result<String, FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        tokio::fs::read_to_string(&path).await
            .map_err(FilesystemError::Io)
    })
}

pub fn write_file(opts: WriteFileOptions) -> BoxedFuture<Result<(), FilesystemError>> {
    let path = opts.path;
    let content = opts.content;
    Box::pin(async move {
        if let Some(p) = std::path::Path::new(&path).parent() {
            if !p.as_os_str().is_empty() {
                tokio::fs::create_dir_all(p).await
                    .map_err(FilesystemError::Io)?;
            }
        }
        tokio::fs::write(path, content).await
            .map_err(FilesystemError::Io)
    })
}

pub fn delete_file(opts: DeleteFileOptions) -> BoxedFuture<Result<(), FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        tokio::fs::remove_file(&path).await
            .map_err(FilesystemError::Io)
    })
}

pub fn create_directory(opts: CreateDirectoryOptions) -> BoxedFuture<Result<(), FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        tokio::fs::create_dir_all(&path).await
            .map_err(FilesystemError::Io)
    })
}

pub fn list_directory(opts: ListDirectoryOptions) -> BoxedFuture<Result<Vec<String>, FilesystemError>> {
    let path = opts.path;
    Box::pin(async move {
        let mut entries = Vec::new();
        let mut reader = tokio::fs::read_dir(&path).await
            .map_err(FilesystemError::Io)?;
        while let Some(entry) = reader.next_entry().await
            .map_err(FilesystemError::Io)? {
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        entries.sort();
        Ok(entries)
    })
}

use std::future::Future;
use std::pin::Pin;

pub type BoxedFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
