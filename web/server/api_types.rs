//! Browser-facing HTTP contract for Yard Web jobs (see docs/web-jobs.md).
//!
//! These types are the single source of truth for the TypeScript client: running
//! `cargo test` regenerates `ui/src/api/types/*.ts` through ts-rs. The job
//! endpoints themselves are implemented later; only the shapes live here.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, export_to = "../ui/src/api/types/")]
pub enum JobAction {
    Deploy,
    Backup,
    Restore,
    ImagesPrune,
    HostCollect,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../ui/src/api/types/")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

/// `POST /api/jobs` body.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct CreateJobRequest {
    pub action: JobAction,
    /// Required for `deploy`, `backup`, `restore`; forbidden for global actions.
    #[serde(default)]
    pub project: Option<String>,
    /// Only for `restore`: the application revision/tag to restore (required).
    #[serde(default)]
    pub revision: Option<String>,
    /// Must be `true` for `restore` and `images-prune` (the CLI's `--yes`).
    #[serde(default)]
    pub confirm: Option<bool>,
}

/// A job as returned by `POST /api/jobs`, `GET /api/jobs` and `GET /api/jobs/{id}`.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct Job {
    pub id: String,
    pub action: JobAction,
    pub project: Option<String>,
    pub revision: Option<String>,
    pub state: JobState,
    /// Basic Auth user as forwarded by the reverse proxy.
    pub actor: String,
    #[ts(type = "number")]
    pub created_at_unix: u64,
    #[ts(type = "number | null")]
    pub started_at_unix: Option<u64>,
    #[ts(type = "number | null")]
    pub finished_at_unix: Option<u64>,
    pub exit_code: Option<i32>,
    /// Current size of the output log in bytes (also the next SSE resume offset).
    #[ts(type = "number")]
    pub output_bytes: u64,
    /// Short human-readable failure reason when `state == "failed"`.
    pub error: Option<String>,
}

/// `GET /api/jobs` query string (all optional).
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct ListJobsQuery {
    pub project: Option<String>,
    pub action: Option<JobAction>,
    pub state: Option<JobState>,
    /// 1..=200, default 50.
    pub limit: Option<u32>,
    /// Opaque cursor returned as `next_cursor`.
    pub cursor: Option<String>,
}

/// `GET /api/jobs` response, newest first.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct JobList {
    pub jobs: Vec<Job>,
    pub next_cursor: Option<String>,
}

/// `GET /api/jobs/{id}/output` query string. `Last-Event-ID` takes precedence.
#[derive(Clone, Debug, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct JobOutputQuery {
    /// Byte offset to resume from (default 0).
    #[ts(type = "number | null")]
    pub offset: Option<u64>,
}

/// Payload of an SSE `output` event. The SSE `id` is `next_offset`.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct OutputChunk {
    #[ts(type = "number")]
    pub offset: u64,
    #[ts(type = "number")]
    pub next_offset: u64,
    /// Always ends on a line boundary except for the final chunk of a job.
    pub text: String,
}

/// `GET /api/logs/stream` query string.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(optional_fields)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct LogsStreamQuery {
    pub project: String,
    pub service: Option<String>,
    /// Docker duration or timestamp, same restrictions as `/api/read?op=logs`.
    pub since: Option<String>,
    /// 1..=1000, default 200.
    pub tail: Option<u32>,
}

/// Payload of a logs SSE `line` event.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct LogLine {
    pub text: String,
}

/// Payload of the final SSE `end` event of a logs stream.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct LogEnd {
    /// `"closed"` (server time limit or container stopped) or `"error"`.
    pub reason: String,
}

/// Machine-readable error body for every non-2xx job/log response.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct ApiError {
    pub error: String,
    pub code: ApiErrorCode,
    /// Present on `409`: the job currently holding the lock.
    pub running_job_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../ui/src/api/types/")]
pub enum ApiErrorCode {
    Refused,
    Forbidden,
    NotFound,
    Conflict,
    ExecutorUnavailable,
    Failed,
}

/// `GET /api/read?op=images-prune-plan` and other reads keep the existing
/// `{"output": "..."}` shape.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../ui/src/api/types/")]
pub struct ReadOutput {
    pub output: String,
}
