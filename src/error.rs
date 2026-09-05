use std::io;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, YardError>;

#[derive(Debug, Error)]
pub enum YardError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("invalid state file: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("project not found: {0}")]
    ProjectNotFound(String),

    #[error("command failed: {program} exited with status {status}\n{stderr}")]
    CommandFailed {
        program: String,
        status: i32,
        stderr: String,
    },

    #[error("health check failed for {0}")]
    HealthCheck(String),

    #[error("no previous deployment is recorded for {0}")]
    NoPreviousRelease(String),

    #[error("image is not available locally: {0}")]
    ImageMissing(String),
}
