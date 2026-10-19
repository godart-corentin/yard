//! Job and live-log endpoints (see docs/web-jobs.md).
//!
//! Mutations go through the ops socket (`start` only); job history and output are
//! read straight from `${YARD_STATE_DIR}/jobs/` through the read-only state mount.
//! Nothing from a request reaches a filesystem path unless it matched the strict
//! job id pattern first.
use std::convert::Infallible;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri};
use axum::response::Response;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::api_types::{
    ApiError, ApiErrorCode, CreateJobRequest, Job, JobAction, JobList, JobState, LogEnd, LogLine,
    OutputChunk,
};
use crate::{load_projects, restore_points, valid_service, valid_since, App, Config};

type Shared = Arc<App>;

const JSON: &str = "application/json; charset=utf-8";
const MAX_BODY: usize = 4096;
const OPS_TIMEOUT: Duration = Duration::from_secs(5);
const OPS_REPLY_LIMIT: usize = 65_536;
const JOB_FILE_LIMIT: u64 = 65_536;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;
const CHUNK_BYTES: usize = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const KEEPALIVE: Duration = Duration::from_secs(15);
/// A job output stream is closed (without `end`, so clients reconnect) after this long.
const OUTPUT_STREAM_MAX: Duration = Duration::from_secs(2 * 60 * 60);
const MAX_OUTPUT_STREAMS: usize = 32;
const LOG_STREAM_MAX: Duration = Duration::from_secs(60 * 60);
const MAX_LOG_STREAMS: usize = 4;
const LOG_TEXT_LIMIT: usize = 16 * 1024;
const LOG_FRAME_LIMIT: u64 = 128 * 1024;

// ---------------------------------------------------------------------------
// Errors

fn json_response(status: StatusCode, body: &impl serde::Serialize) -> Response {
    let mut bytes = serde_json::to_vec(body).unwrap_or_default();
    bytes.push(b'\n');
    crate::http::respond(status, JSON, "no-store", bytes)
}

fn api_error(status: StatusCode, code: ApiErrorCode, running_job_id: Option<String>) -> Response {
    let message = match code {
        ApiErrorCode::Refused => "Request refused",
        ApiErrorCode::Forbidden => "Forbidden",
        ApiErrorCode::NotFound => "Job not found",
        ApiErrorCode::Conflict => "A job is already running",
        ApiErrorCode::ExecutorUnavailable => "Executor unavailable",
        ApiErrorCode::Failed => "Internal error",
    };
    json_response(
        status,
        &ApiError {
            error: message.to_owned(),
            code,
            running_job_id,
        },
    )
}

fn refused() -> Response {
    api_error(StatusCode::BAD_REQUEST, ApiErrorCode::Refused, None)
}

fn forbidden() -> Response {
    api_error(StatusCode::FORBIDDEN, ApiErrorCode::Forbidden, None)
}

fn not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, ApiErrorCode::NotFound, None)
}

fn unavailable() -> Response {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        ApiErrorCode::ExecutorUnavailable,
        None,
    )
}

fn internal() -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        ApiErrorCode::Failed,
        None,
    )
}

fn too_many_streams() -> Response {
    api_error(StatusCode::TOO_MANY_REQUESTS, ApiErrorCode::Refused, None)
}

// ---------------------------------------------------------------------------
// Validation helpers

pub fn valid_job_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 25
        && bytes[..8].iter().all(u8::is_ascii_digit)
        && bytes[8] == b'T'
        && bytes[9..15].iter().all(u8::is_ascii_digit)
        && bytes[15] == b'Z'
        && bytes[16] == b'-'
        && bytes[17..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_actor(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'@' | b'-'))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(a ^ b);
    }
    difference == 0
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// `host[:port]` of an `Origin` value, lowercased; `None` for `null` or junk.
fn origin_authority(origin: &str) -> Option<String> {
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    (!rest.is_empty() && !rest.contains(['/', '?', '#', '@', ' ']))
        .then(|| rest.to_ascii_lowercase())
}

/// CSRF and identity gate for mutating requests. Returns the validated actor.
fn authorize_mutation(headers: &HeaderMap, config: &Config) -> Result<String, ()> {
    if header_str(headers, "x-yard-request") != Some("1") {
        return Err(());
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        if site.as_bytes() != b"same-origin" {
            return Err(());
        }
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let expected = header_str(headers, "x-forwarded-host")
            .and_then(|value| value.split(',').next())
            .or_else(|| header_str(headers, "host"))
            .map(|value| value.trim().to_ascii_lowercase());
        let actual = origin.to_str().ok().and_then(origin_authority);
        if expected.is_none() || actual != expected {
            return Err(());
        }
    }
    let token = config.proxy_token.as_deref().ok_or(())?;
    let provided = header_str(headers, "x-yard-proxy-token").ok_or(())?;
    if !constant_time_eq(token.as_bytes(), provided.as_bytes()) {
        return Err(());
    }
    let actor = header_str(headers, "x-yard-user").ok_or(())?;
    valid_actor(actor).then(|| actor.to_owned()).ok_or(())
}

// ---------------------------------------------------------------------------
// POST /api/jobs

fn is_global(action: JobAction) -> bool {
    matches!(action, JobAction::ImagesPrune | JobAction::HostCollect)
}

fn action_name(action: JobAction) -> &'static str {
    match action {
        JobAction::Deploy => "deploy",
        JobAction::Backup => "backup",
        JobAction::Restore => "restore",
        JobAction::ImagesPrune => "images-prune",
        JobAction::HostCollect => "host-collect",
    }
}

/// Validates a request against the contract and the configured projects.
/// Returns the `start` frame to send to the executor.
fn build_start(request: &CreateJobRequest, actor: &str, config: &Config) -> Option<Value> {
    let action = request.action;
    let needs_confirm = matches!(action, JobAction::Restore | JobAction::ImagesPrune);
    if needs_confirm && request.confirm != Some(true) {
        return None;
    }
    if request.revision.is_some() && action != JobAction::Restore {
        return None;
    }
    let mut frame = json!({"v": 1, "op": "start", "action": action_name(action), "actor": actor});
    if is_global(action) {
        if request.project.is_some() {
            return None;
        }
    } else {
        let name = request.project.as_deref()?;
        if !valid_project_name(name) {
            return None;
        }
        let projects = load_projects(&config.projects_dir, &config.state_dir);
        let project = projects.iter().find(|item| item.name == name)?;
        if project.url_monitor || project.config_error.is_some() {
            return None;
        }
        frame["project"] = json!(name);
        if action == JobAction::Restore {
            let revision = request.revision.as_deref()?;
            let known = restore_points(
                &project.release,
                &project.previous_release,
                &project.pending_release,
            );
            if !known.iter().any(|point| {
                point.id == revision || point.id.strip_prefix("release:") == Some(revision)
            }) {
                return None;
            }
            frame["revision"] = json!(revision);
        }
    }
    if needs_confirm {
        frame["confirm"] = json!(true);
    }
    Some(frame)
}

enum OpsFailure {
    Unavailable,
    Protocol,
}

/// One request, one reply line, with a hard 5 s deadline and a reply size cap.
fn ops_call(socket: &Path, frame: &Value) -> Result<Value, OpsFailure> {
    let payload = serde_json::to_vec(frame).map_err(|_| OpsFailure::Protocol)?;
    if payload.len() + 1 > MAX_BODY {
        return Err(OpsFailure::Protocol);
    }
    let deadline = Instant::now() + OPS_TIMEOUT;
    let mut stream = UnixStream::connect(socket).map_err(|_| OpsFailure::Unavailable)?;
    stream
        .set_write_timeout(Some(OPS_TIMEOUT))
        .map_err(|_| OpsFailure::Unavailable)?;
    stream
        .write_all(&payload)
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|_| OpsFailure::Unavailable)?;
    let mut reply = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(OpsFailure::Unavailable);
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|_| OpsFailure::Unavailable)?;
        let count = stream
            .read(&mut buffer)
            .map_err(|_| OpsFailure::Unavailable)?;
        if count == 0 {
            break;
        }
        reply.extend_from_slice(&buffer[..count]);
        if reply.len() > OPS_REPLY_LIMIT {
            return Err(OpsFailure::Protocol);
        }
        if reply.contains(&b'\n') {
            break;
        }
    }
    let line = reply.split(|byte| *byte == b'\n').next().unwrap_or(&[]);
    serde_json::from_slice(line).map_err(|_| OpsFailure::Protocol)
}

fn map_start_reply(reply: Result<Value, OpsFailure>) -> Response {
    let reply = match reply {
        Ok(reply) => reply,
        Err(OpsFailure::Unavailable) => return unavailable(),
        // A malformed executor reply is reported like an unavailable executor.
        Err(OpsFailure::Protocol) => return unavailable(),
    };
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        let job = reply
            .get("job")
            .and_then(|value| serde_json::from_value::<Job>(value.clone()).ok())
            .and_then(sanitize_job);
        return match job {
            Some(job) => {
                let mut response = json_response(StatusCode::ACCEPTED, &job);
                if let Ok(location) = HeaderValue::from_str(&format!("/api/jobs/{}", job.id)) {
                    response.headers_mut().insert(header::LOCATION, location);
                }
                response
            }
            None => internal(),
        };
    }
    match reply.get("error").and_then(Value::as_str) {
        Some("refused") => refused(),
        Some("conflict") => {
            let running = reply
                .get("running_job_id")
                .and_then(Value::as_str)
                .filter(|id| valid_job_id(id))
                .map(str::to_owned);
            api_error(StatusCode::CONFLICT, ApiErrorCode::Conflict, running)
        }
        _ => internal(),
    }
}

pub async fn create_job(State(app): State<Shared>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Ok(actor) = authorize_mutation(&parts.headers, &app.config) else {
        return forbidden();
    };
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BODY).await else {
        return refused();
    };
    let Ok(job_request) = serde_json::from_slice::<CreateJobRequest>(&bytes) else {
        return refused();
    };
    tokio::task::spawn_blocking(move || {
        let Some(frame) = build_start(&job_request, &actor, &app.config) else {
            return refused();
        };
        map_start_reply(ops_call(&app.config.ops_socket, &frame))
    })
    .await
    .unwrap_or_else(|_| internal())
}

// ---------------------------------------------------------------------------
// Reading jobs from the state directory

fn jobs_dir(config: &Config) -> PathBuf {
    config.state_dir.join("jobs")
}

fn clean_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect()
}

/// Keeps only well-formed fields of a job record; `None` for malformed ids.
fn sanitize_job(mut job: Job) -> Option<Job> {
    if !valid_job_id(&job.id) {
        return None;
    }
    if job
        .project
        .as_deref()
        .is_some_and(|p| !valid_project_name(p))
    {
        job.project = None;
    }
    job.revision = job
        .revision
        .map(|value| clean_text(&value, 200))
        .filter(|value| !value.is_empty());
    if !valid_actor(&job.actor) {
        job.actor = "unknown".to_owned();
    }
    job.error = job.error.map(|value| clean_text(&value, 300));
    Some(job)
}

fn read_job(dir: &Path, id: &str) -> Option<Job> {
    if !valid_job_id(id) {
        return None;
    }
    let path = dir.join(format!("{id}.json"));
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > JOB_FILE_LIMIT {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let job: Job = serde_json::from_slice(&bytes).ok()?;
    if job.id != id {
        return None;
    }
    sanitize_job(job)
}

struct ListFilter {
    project: Option<String>,
    action: Option<JobAction>,
    state: Option<JobState>,
    limit: usize,
    cursor: Option<String>,
}

fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Option<T> {
    serde_json::from_value(Value::String(value.to_owned())).ok()
}

fn parse_list_query(uri: &Uri) -> Option<ListFilter> {
    let mut filter = ListFilter {
        project: None,
        action: None,
        state: None,
        limit: DEFAULT_LIMIT,
        cursor: None,
    };
    let mut seen = std::collections::BTreeSet::new();
    let url = reqwest::Url::parse(&format!("http://localhost{uri}")).ok()?;
    for (key, value) in url.query_pairs() {
        if !seen.insert(key.to_string()) {
            return None;
        }
        match key.as_ref() {
            "project" => {
                if !valid_project_name(&value) {
                    return None;
                }
                filter.project = Some(value.into_owned());
            }
            "action" => filter.action = Some(parse_enum(&value)?),
            "state" => filter.state = Some(parse_enum(&value)?),
            "limit" => {
                let limit: usize = value.parse().ok()?;
                if !(1..=MAX_LIMIT).contains(&limit) {
                    return None;
                }
                filter.limit = limit;
            }
            "cursor" => {
                if !valid_job_id(&value) {
                    return None;
                }
                filter.cursor = Some(value.into_owned());
            }
            _ => return None,
        }
    }
    Some(filter)
}

fn list_jobs_in(dir: &Path, filter: &ListFilter) -> JobList {
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    let id = name.strip_suffix(".json")?;
                    valid_job_id(id).then(|| id.to_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort_unstable_by(|a, b| b.cmp(a));
    let mut jobs = Vec::new();
    let mut more = false;
    for id in ids {
        if filter
            .cursor
            .as_deref()
            .is_some_and(|cursor| id.as_str() >= cursor)
        {
            continue;
        }
        let Some(job) = read_job(dir, &id) else {
            continue;
        };
        if filter
            .project
            .as_ref()
            .is_some_and(|p| job.project.as_ref() != Some(p))
            || filter.action.is_some_and(|a| job.action != a)
            || filter.state.is_some_and(|s| job.state != s)
        {
            continue;
        }
        if jobs.len() == filter.limit {
            more = true;
            break;
        }
        jobs.push(job);
    }
    let next_cursor = more
        .then(|| jobs.last().map(|job| job.id.clone()))
        .flatten();
    JobList { jobs, next_cursor }
}

pub async fn list_jobs(State(app): State<Shared>, uri: Uri) -> Response {
    let Some(filter) = parse_list_query(&uri) else {
        return refused();
    };
    let dir = jobs_dir(&app.config);
    match tokio::task::spawn_blocking(move || list_jobs_in(&dir, &filter)).await {
        Ok(list) => json_response(StatusCode::OK, &list),
        Err(_) => internal(),
    }
}

pub async fn get_job(State(app): State<Shared>, UrlPath(id): UrlPath<String>) -> Response {
    let dir = jobs_dir(&app.config);
    match tokio::task::spawn_blocking(move || read_job(&dir, &id)).await {
        Ok(Some(job)) => json_response(StatusCode::OK, &job),
        Ok(None) => not_found(),
        Err(_) => internal(),
    }
}

// ---------------------------------------------------------------------------
// SSE plumbing

#[derive(Default)]
pub struct Streams {
    outputs: Arc<AtomicUsize>,
    logs: Arc<AtomicUsize>,
}

struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn acquire(counter: &Arc<AtomicUsize>, max: usize) -> Option<Slot> {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value < max).then_some(value + 1)
            })
            .ok()
            .map(|_| Slot(Arc::clone(counter)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct EventStream(mpsc::Receiver<Bytes>);

impl futures_core::Stream for EventStream {
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|item| item.map(Ok))
    }
}

fn sse_response(receiver: mpsc::Receiver<Bytes>) -> Response {
    let mut response = Response::new(Body::from_stream(EventStream(receiver)));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    response
}

fn event(name: &str, id: Option<u64>, data: &impl serde::Serialize) -> Bytes {
    let mut text = format!("event: {name}\n");
    if let Some(id) = id {
        text.push_str(&format!("id: {id}\n"));
    }
    // serde_json output is a single line: newlines inside strings are escaped.
    text.push_str(&format!(
        "data: {}\n\n",
        serde_json::to_string(data).unwrap_or_default()
    ));
    Bytes::from(text)
}

fn keepalive() -> Bytes {
    Bytes::from_static(b": keepalive\n\n")
}

// ---------------------------------------------------------------------------
// GET /api/jobs/{id}/output

fn is_terminal(state: JobState) -> bool {
    matches!(state, JobState::Succeeded | JobState::Failed)
}

/// Number of trailing bytes forming an incomplete (but so far valid) UTF-8 sequence.
fn incomplete_tail(buf: &[u8]) -> usize {
    for back in 1..=buf.len().min(3) {
        let byte = buf[buf.len() - back];
        if byte & 0xC0 == 0x80 {
            continue;
        }
        let needed = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return 0,
        };
        return if needed > back { back } else { 0 };
    }
    0
}

#[derive(Debug)]
enum Step {
    Chunk(OutputChunk),
    Idle,
    Finished,
}

/// Reads the next chunk of a job log. `terminal` means the job is finished, so
/// the log is complete and the trailing partial line may be flushed.
fn read_step(path: &Path, offset: u64, terminal: bool) -> std::io::Result<Step> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(if terminal { Step::Finished } else { Step::Idle });
        }
        Err(error) => return Err(error),
    };
    let length = file.metadata()?.len();
    let offset = offset.min(length);
    if offset >= length {
        return Ok(if terminal { Step::Finished } else { Step::Idle });
    }
    std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(offset))?;
    let want = (length - offset).min(CHUNK_BYTES as u64) as usize;
    let mut buf = vec![0u8; want];
    file.read_exact(&mut buf)?;
    let is_final = terminal && offset + want as u64 == length;
    let cut = if is_final {
        buf.len()
    } else if let Some(position) = buf.iter().rposition(|byte| *byte == b'\n') {
        position + 1
    } else if buf.len() == CHUNK_BYTES {
        // A single very long line: split on a character boundary.
        match buf.len() - incomplete_tail(&buf) {
            0 => buf.len(),
            cut => cut,
        }
    } else {
        return Ok(Step::Idle);
    };
    Ok(Step::Chunk(OutputChunk {
        offset,
        next_offset: offset + cut as u64,
        text: String::from_utf8_lossy(&buf[..cut]).into_owned(),
    }))
}

async fn stream_output(dir: PathBuf, first: Job, mut offset: u64, tx: mpsc::Sender<Bytes>) {
    let log = dir.join(format!("{}.log", first.id));
    let started = Instant::now();
    let mut last_write = Instant::now();
    let mut job = first;
    let mut sent: Option<(JobState, u64)> = None;
    let mut first_pass = true;
    // Job metadata is read before the log so a terminal state implies a complete log.
    loop {
        if tx.is_closed() || started.elapsed() > OUTPUT_STREAM_MAX {
            return;
        }
        if !first_pass {
            let (dir, id) = (dir.clone(), job.id.clone());
            if let Ok(Some(fresh)) = tokio::task::spawn_blocking(move || read_job(&dir, &id)).await
            {
                job = fresh;
            }
        }
        first_pass = false;
        let terminal = is_terminal(job.state);
        if sent != Some((job.state, job.output_bytes)) {
            sent = Some((job.state, job.output_bytes));
            if tx.send(event("state", None, &job)).await.is_err() {
                return;
            }
            last_write = Instant::now();
        }
        loop {
            let path = log.clone();
            let step = tokio::task::spawn_blocking(move || read_step(&path, offset, terminal))
                .await
                .unwrap_or_else(|_| Err(std::io::Error::other("join")));
            match step {
                Ok(Step::Chunk(chunk)) => {
                    offset = chunk.next_offset;
                    let data = event("output", Some(chunk.next_offset), &chunk);
                    if tx.send(data).await.is_err() {
                        return;
                    }
                    last_write = Instant::now();
                }
                Ok(Step::Finished) => {
                    let _ = tx.send(event("end", None, &job)).await;
                    return;
                }
                Ok(Step::Idle) => break,
                Err(_) => {
                    if terminal {
                        let _ = tx.send(event("end", None, &job)).await;
                    }
                    if terminal || tx.is_closed() {
                        return;
                    }
                    break;
                }
            }
        }
        if last_write.elapsed() >= KEEPALIVE {
            if tx.send(keepalive()).await.is_err() {
                return;
            }
            last_write = Instant::now();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn parse_offset(headers: &HeaderMap, uri: &Uri) -> Option<u64> {
    let mut query_offset = None;
    if let Some(query) = uri.query() {
        let url = reqwest::Url::parse(&format!("http://localhost/?{query}")).ok()?;
        for (key, value) in url.query_pairs() {
            if key != "offset" || query_offset.is_some() {
                return None;
            }
            query_offset = Some(value.parse::<u64>().ok()?);
        }
    }
    match headers.get("last-event-id") {
        Some(value) => value.to_str().ok()?.trim().parse::<u64>().ok(),
        None => Some(query_offset.unwrap_or(0)),
    }
}

pub async fn job_output(
    State(app): State<Shared>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let Some(offset) = parse_offset(&headers, &uri) else {
        return refused();
    };
    let dir = jobs_dir(&app.config);
    let lookup = (dir.clone(), id);
    let job = match tokio::task::spawn_blocking(move || read_job(&lookup.0, &lookup.1)).await {
        Ok(Some(job)) => job,
        Ok(None) => return not_found(),
        Err(_) => return internal(),
    };
    let Some(slot) = Slot::acquire(&app.streams.outputs, MAX_OUTPUT_STREAMS) else {
        return too_many_streams();
    };
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        let _slot = slot;
        stream_output(dir, job, offset, tx).await;
    });
    sse_response(rx)
}

// ---------------------------------------------------------------------------
// GET /api/logs/stream

struct LogsParams {
    project: String,
    service: Option<String>,
    since: Option<String>,
    tail: u32,
}

fn parse_logs_query(uri: &Uri) -> Option<LogsParams> {
    let url = reqwest::Url::parse(&format!("http://localhost{uri}")).ok()?;
    let (mut project, mut service, mut since, mut tail) = (None, None, None, None);
    for (key, value) in url.query_pairs() {
        let slot = match key.as_ref() {
            "project" => &mut project,
            "service" => &mut service,
            "since" => &mut since,
            "tail" => &mut tail,
            _ => return None,
        };
        if slot.replace(value.into_owned()).is_some() {
            return None;
        }
    }
    let tail = match tail {
        Some(value) => value
            .parse::<u32>()
            .ok()
            .filter(|n| (1..=1000).contains(n))?,
        None => 200,
    };
    Some(LogsParams {
        project: project.filter(|name| valid_project_name(name))?,
        service,
        since,
        tail,
    })
}

fn logs_follow_frame(params: &LogsParams, config: &Config) -> Option<Value> {
    let projects = load_projects(&config.projects_dir, &config.state_dir);
    let project = projects.iter().find(|item| item.name == params.project)?;
    if project.service_names.is_empty()
        || project.config_error.is_some()
        || !valid_service(project, params.service.as_deref())
        || !valid_since(params.since.as_deref())
    {
        return None;
    }
    Some(json!({
        "op": "logs-follow",
        "project": params.project,
        "service": params.service,
        "tail": params.tail,
        "since": params.since,
    }))
}

fn truncate_text(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut cut = limit;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    text
}

fn end_event(reason: &str) -> Bytes {
    event(
        "end",
        None,
        &LogEnd {
            reason: reason.to_owned(),
        },
    )
}

async fn relay_logs(socket: tokio::net::UnixStream, tx: mpsc::Sender<Bytes>) {
    let mut reader = tokio::io::BufReader::new(socket);
    let deadline = tokio::time::Instant::now() + LOG_STREAM_MAX;
    let mut keepalive_timer = tokio::time::interval(KEEPALIVE);
    keepalive_timer.reset();
    let mut line = Vec::new();
    loop {
        line.clear();
        let outcome = {
            let mut limited = (&mut reader).take(LOG_FRAME_LIMIT);
            let read = limited.read_until(b'\n', &mut line);
            tokio::pin!(read);
            loop {
                tokio::select! {
                    result = &mut read => break Some(result),
                    _ = tx.closed() => return,
                    _ = tokio::time::sleep_until(deadline) => break None,
                    _ = keepalive_timer.tick() => {
                        if tx.send(keepalive()).await.is_err() {
                            return;
                        }
                    }
                }
            }
        };
        let Some(result) = outcome else {
            let _ = tx.send(end_event("closed")).await;
            return;
        };
        let complete = line.last() == Some(&b'\n');
        let frame: Option<Value> = match result {
            Ok(count) if count > 0 && complete => serde_json::from_slice(&line).ok(),
            _ => None,
        };
        let Some(frame) = frame else {
            let _ = tx.send(end_event("error")).await;
            return;
        };
        if let Some(text) = frame.get("line").and_then(Value::as_str) {
            let text = truncate_text(text.to_owned(), LOG_TEXT_LIMIT);
            if tx
                .send(event("line", None, &LogLine { text }))
                .await
                .is_err()
            {
                return;
            }
        } else {
            let reason = match frame.get("end").and_then(Value::as_str) {
                Some("closed") => "closed",
                _ => "error",
            };
            let _ = tx.send(end_event(reason)).await;
            return;
        }
    }
}

pub async fn logs_stream(State(app): State<Shared>, uri: Uri) -> Response {
    let Some(params) = parse_logs_query(&uri) else {
        return refused();
    };
    let state = Arc::clone(&app);
    let frame = tokio::task::spawn_blocking(move || logs_follow_frame(&params, &state.config))
        .await
        .ok()
        .flatten();
    let Some(frame) = frame else {
        return refused();
    };
    let Some(slot) = Slot::acquire(&app.streams.logs, MAX_LOG_STREAMS) else {
        return too_many_streams();
    };
    let connect = tokio::net::UnixStream::connect(&app.config.read_socket);
    let Ok(Ok(mut socket)) = tokio::time::timeout(Duration::from_secs(5), connect).await else {
        return unavailable();
    };
    let mut request = serde_json::to_vec(&frame).unwrap_or_default();
    request.push(b'\n');
    let sent = tokio::time::timeout(Duration::from_secs(5), socket.write_all(&request)).await;
    if !matches!(sent, Ok(Ok(()))) {
        return unavailable();
    }
    let (tx, rx) = mpsc::channel(32);
    tokio::spawn(async move {
        let _slot = slot;
        relay_logs(socket, tx).await;
    });
    sse_response(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http;
    use std::fs;
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc as std_mpsc;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TOKEN: &str = "s3cret-proxy-token";
    const ID_A: &str = "20261019T142501Z-3fa9c01b";
    const ID_B: &str = "20261019T150000Z-00000001";
    const ID_C: &str = "20261020T080000Z-abcdef12";

    fn temp_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("yw-{label}-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    struct Fixture {
        root: PathBuf,
        app: Shared,
    }

    impl Fixture {
        fn new(label: &str, token: Option<&str>) -> Fixture {
            let root = temp_dir(label);
            for dir in ["projects", "state/jobs"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            fs::write(
                root.join("projects/demo.toml"),
                "[compose]\nservices = ['api']\n",
            )
            .unwrap();
            fs::write(
                root.join("projects/mon.toml"),
                "[deployment]\nhealth_url = 'http://localhost/up'\n",
            )
            .unwrap();
            fs::write(
                root.join("state/demo.json"),
                r#"{"current":{"revision":"abcd1234","tag":"cur","deployed_at_unix":100},"previous":{"revision":"abcd0000","tag":"old","deployed_at_unix":90}}"#,
            )
            .unwrap();
            let config = Config {
                host: "127.0.0.1".into(),
                port: 0,
                projects_dir: root.join("projects"),
                state_dir: root.join("state"),
                static_dir: root.clone(),
                check_timeout: Duration::from_secs(1),
                cache_duration: Duration::from_secs(1),
                host_max_age_seconds: 300,
                read_socket: root.join("read.sock"),
                ops_socket: root.join("ops.sock"),
                proxy_token: token.map(str::to_owned),
            };
            Fixture {
                app: Arc::new(App::new(config).unwrap()),
                root,
            }
        }

        fn jobs(&self) -> PathBuf {
            self.root.join("state/jobs")
        }

        fn write_job(
            &self,
            id: &str,
            action: &str,
            state: &str,
            project: Option<&str>,
            bytes: u64,
        ) {
            let job = json!({
                "v": 1, "id": id, "action": action, "project": project, "revision": null,
                "state": state, "actor": "corentin", "created_at_unix": 1, "started_at_unix": 2,
                "finished_at_unix": if state == "running" { Value::Null } else { json!(3) },
                "exit_code": if state == "succeeded" { json!(0) } else { Value::Null },
                "output_bytes": bytes, "error": null,
            });
            fs::write(self.jobs().join(format!("{id}.json")), job.to_string()).unwrap();
        }

        /// Serves exactly one ops connection with `reply`; yields the received line.
        fn fake_ops(&self, reply: String) -> std_mpsc::Receiver<String> {
            let listener = UnixListener::bind(self.root.join("ops.sock")).unwrap();
            let (tx, rx) = std_mpsc::channel();
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut received = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
                    received.push(byte[0]);
                }
                tx.send(String::from_utf8(received).unwrap()).unwrap();
                stream.write_all(reply.as_bytes()).unwrap();
                stream.write_all(b"\n").unwrap();
            });
            rx
        }

        fn send(&self, request: &str) -> (String, String) {
            let raw = http::exchange(&self.app, request);
            let (head, body) = raw.split_once("\r\n\r\n").unwrap();
            (head.to_owned(), body.to_owned())
        }

        fn post(&self, headers: &[(&str, &str)], body: &str) -> (String, String) {
            let mut raw =
                String::from("POST /api/jobs HTTP/1.1\r\nHost: yard.test\r\nConnection: close\r\n");
            for (name, value) in headers {
                raw.push_str(&format!("{name}: {value}\r\n"));
            }
            raw.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
            self.send(&raw)
        }

        fn get(&self, path: &str, extra: &str) -> (String, String) {
            self.send(&format!(
                "GET {path} HTTP/1.1\r\nHost: yard.test\r\nConnection: close\r\n{extra}\r\n"
            ))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn good_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("X-Yard-Request", "1"),
            ("X-Yard-User", "corentin"),
            ("X-Yard-Proxy-Token", TOKEN),
            ("Content-Type", "application/json"),
        ]
    }

    fn status_line(head: &str) -> &str {
        head.lines().next().unwrap()
    }

    fn json_body(body: &str) -> Value {
        // The body may be chunk-encoded; strip chunk framing when present.
        let start = body.find('{').unwrap();
        let end = body.rfind('}').unwrap();
        serde_json::from_str(&body[start..=end]).unwrap()
    }

    fn ok_job(id: &str, action: &str, project: Option<&str>) -> String {
        json!({"ok": true, "job": {
            "id": id, "action": action, "project": project, "revision": null,
            "state": "running", "actor": "corentin", "created_at_unix": 1,
            "started_at_unix": 2, "finished_at_unix": null, "exit_code": null,
            "output_bytes": 0, "error": null}})
        .to_string()
    }

    #[test]
    fn id_pattern_is_strict() {
        assert!(valid_job_id(ID_A));
        for bad in [
            "",
            "../etc/passwd",
            "20261019T142501Z-3FA9C01B",
            "20261019T142501Z-3fa9c01",
            "20261019T142501Z-3fa9c01bb",
            "20261019T142501Z-3fa9c01b.json",
            "20261019T142501Z-3fa9c01b/../x",
            "2026101ST142501Z-3fa9c01b",
        ] {
            assert!(!valid_job_id(bad), "{bad}");
        }
    }

    #[test]
    fn helpers_behave() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert_eq!(
            origin_authority("https://Yard.Test"),
            Some("yard.test".into())
        );
        assert_eq!(origin_authority("http://h:8088"), Some("h:8088".into()));
        assert_eq!(origin_authority("null"), None);
        assert_eq!(origin_authority("https://evil.test/@yard.test"), None);
        assert_eq!(incomplete_tail("é".as_bytes()), 0);
        assert_eq!(incomplete_tail(&"é".as_bytes()[..1]), 1);
        assert_eq!(incomplete_tail(&"€".as_bytes()[..2]), 2);
        assert_eq!(incomplete_tail(&"😀".as_bytes()[..3]), 3);
        assert_eq!(incomplete_tail(b"abc"), 0);
        assert_eq!(incomplete_tail(&[0xff]), 0);
    }

    #[test]
    fn read_step_cuts_on_lines_and_flushes_when_terminal() {
        let dir = temp_dir("step");
        let log = dir.join("x.log");
        fs::write(&log, "one\ntwo\npart").unwrap();
        let Step::Chunk(chunk) = read_step(&log, 0, false).unwrap() else {
            panic!("expected chunk");
        };
        assert_eq!(
            (chunk.offset, chunk.next_offset, chunk.text.as_str()),
            (0, 8, "one\ntwo\n")
        );
        assert!(matches!(read_step(&log, 8, false).unwrap(), Step::Idle));
        let Step::Chunk(rest) = read_step(&log, 8, true).unwrap() else {
            panic!("expected chunk");
        };
        assert_eq!((rest.next_offset, rest.text.as_str()), (12, "part"));
        assert!(matches!(read_step(&log, 12, true).unwrap(), Step::Finished));
        // Offset beyond the end starts at the end.
        assert!(matches!(read_step(&log, 999, false).unwrap(), Step::Idle));
        assert!(matches!(
            read_step(&dir.join("none.log"), 0, true).unwrap(),
            Step::Finished
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_step_never_splits_a_character() {
        let dir = temp_dir("utf8");
        let log = dir.join("x.log");
        // One long line: 3-byte characters straddle the 64 KiB chunk boundary.
        let line = "€".repeat(30_000);
        fs::write(&log, format!("{line}\n")).unwrap();
        let total = fs::metadata(&log).unwrap().len();
        let (mut offset, mut text) = (0, String::new());
        while let Step::Chunk(chunk) = read_step(&log, offset, true).unwrap() {
            assert!(!chunk.text.contains('\u{fffd}'));
            offset = chunk.next_offset;
            text.push_str(&chunk.text);
        }
        assert_eq!(offset, total);
        assert_eq!(text, format!("{line}\n"));
        // Invalid bytes are replaced, offsets stay byte based.
        fs::write(&log, b"a\xffb\n").unwrap();
        let Step::Chunk(chunk) = read_step(&log, 0, false).unwrap() else {
            panic!("expected chunk");
        };
        assert_eq!(
            (chunk.next_offset, chunk.text.as_str()),
            (4, "a\u{fffd}b\n")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refuses_csrf_failures_and_missing_proxy_trust() {
        let fixture = Fixture::new("csrf", Some(TOKEN));
        let body = r#"{"action":"deploy","project":"demo"}"#;
        let without = |name: &str| -> Vec<(&str, &str)> {
            good_headers()
                .into_iter()
                .filter(|(n, _)| *n != name)
                .collect()
        };
        let mut cases: Vec<Vec<(&str, &str)>> = vec![
            without("X-Yard-Request"),
            without("X-Yard-User"),
            without("X-Yard-Proxy-Token"),
        ];
        let mut wrong_token = without("X-Yard-Proxy-Token");
        wrong_token.push(("X-Yard-Proxy-Token", "nope"));
        cases.push(wrong_token);
        let mut bad_user = without("X-Yard-User");
        bad_user.push(("X-Yard-User", "bad user;rm"));
        cases.push(bad_user);
        let mut cross_site = good_headers();
        cross_site.push(("Sec-Fetch-Site", "cross-site"));
        cases.push(cross_site);
        let mut other_origin = good_headers();
        other_origin.push(("Origin", "https://evil.test"));
        cases.push(other_origin);
        let mut wrong_marker = without("X-Yard-Request");
        wrong_marker.push(("X-Yard-Request", "yes"));
        cases.push(wrong_marker);
        for headers in cases {
            let (head, response) = fixture.post(&headers, body);
            assert!(status_line(&head).contains("403"), "{headers:?}: {head}");
            let json = json_body(&response);
            assert_eq!(json["code"], "forbidden");
            assert!(!response.contains(TOKEN));
            assert!(head
                .to_ascii_lowercase()
                .contains("cache-control: no-store"));
        }
        // No token configured: every mutation is forbidden even with valid headers.
        let open = Fixture::new("csrf-open", None);
        let (head, _) = open.post(&good_headers(), body);
        assert!(status_line(&head).contains("403"));
    }

    #[test]
    fn same_origin_browser_requests_pass_csrf() {
        let fixture = Fixture::new("origin", Some(TOKEN));
        let received = fixture.fake_ops(ok_job(ID_A, "deploy", Some("demo")));
        let mut headers = good_headers();
        headers.extend([
            ("Sec-Fetch-Site", "same-origin"),
            ("Origin", "https://yard.test"),
            ("X-Forwarded-Host", "yard.test"),
        ]);
        let (head, body) = fixture.post(&headers, r#"{"action":"deploy","project":"demo"}"#);
        assert!(status_line(&head).contains("202"), "{head}");
        assert!(head
            .to_ascii_lowercase()
            .contains(&format!("location: /api/jobs/{ID_A}").to_ascii_lowercase()));
        assert_eq!(json_body(&body)["id"], ID_A);
        let sent: Value = serde_json::from_str(&received.recv().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"v":1,"op":"start","action":"deploy","project":"demo","actor":"corentin"})
        );
    }

    #[test]
    fn validates_arguments_before_touching_the_executor() {
        let fixture = Fixture::new("validate", Some(TOKEN));
        // No ops socket exists: any 503 here would mean validation let the request through.
        let cases = [
            "not json",
            r#"{"action":"nuke"}"#,
            r#"{"action":"deploy"}"#,
            r#"{"action":"deploy","project":"demo","extra":1}"#,
            r#"{"action":"deploy","project":"../etc"}"#,
            r#"{"action":"deploy","project":"missing"}"#,
            r#"{"action":"deploy","project":"mon"}"#,
            r#"{"action":"deploy","project":"host"}"#,
            r#"{"action":"deploy","project":"demo","revision":"cur"}"#,
            r#"{"action":"restore","project":"demo","confirm":true}"#,
            r#"{"action":"restore","project":"demo","revision":"cur"}"#,
            r#"{"action":"restore","project":"demo","revision":"cur","confirm":false}"#,
            r#"{"action":"restore","project":"demo","revision":"unknown","confirm":true}"#,
            r#"{"action":"images-prune"}"#,
            r#"{"action":"images-prune","confirm":true,"project":"demo"}"#,
            r#"{"action":"host-collect","project":"demo"}"#,
        ];
        for body in cases {
            let (head, response) = fixture.post(&good_headers(), body);
            assert!(status_line(&head).contains("400"), "{body}: {head}");
            assert_eq!(json_body(&response)["code"], "refused", "{body}");
        }
    }

    #[test]
    fn forwards_restore_and_global_actions() {
        let fixture = Fixture::new("forward", Some(TOKEN));
        let received = fixture.fake_ops(ok_job(ID_A, "restore", Some("demo")));
        let (head, _) = fixture.post(
            &good_headers(),
            r#"{"action":"restore","project":"demo","revision":"release:old","confirm":true}"#,
        );
        assert!(status_line(&head).contains("202"), "{head}");
        let sent: Value = serde_json::from_str(&received.recv().unwrap()).unwrap();
        assert_eq!(sent["revision"], "release:old");
        assert_eq!(sent["confirm"], true);
        fs::remove_file(fixture.root.join("ops.sock")).unwrap();
        let received = fixture.fake_ops(ok_job(ID_B, "images-prune", None));
        let (head, _) = fixture.post(
            &good_headers(),
            r#"{"action":"images-prune","confirm":true}"#,
        );
        assert!(status_line(&head).contains("202"), "{head}");
        let sent: Value = serde_json::from_str(&received.recv().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"v":1,"op":"start","action":"images-prune","actor":"corentin","confirm":true})
        );
    }

    #[test]
    fn maps_executor_errors() {
        let fixture = Fixture::new("map", Some(TOKEN));
        let body = r#"{"action":"backup","project":"demo"}"#;
        let (head, response) = fixture.post(&good_headers(), body);
        assert!(status_line(&head).contains("503"), "{head}");
        assert_eq!(json_body(&response)["code"], "executor_unavailable");

        let reply =
            json!({"ok":false,"error":"conflict","message":"secret detail","running_job_id":ID_A});
        let _rx = fixture.fake_ops(reply.to_string());
        let (head, response) = fixture.post(&good_headers(), body);
        assert!(status_line(&head).contains("409"), "{head}");
        let json = json_body(&response);
        assert_eq!(json["code"], "conflict");
        assert_eq!(json["running_job_id"], ID_A);
        assert!(!response.contains("secret detail"));
        fs::remove_file(fixture.root.join("ops.sock")).unwrap();

        let _rx =
            fixture.fake_ops(json!({"ok":false,"error":"refused","message":"raw"}).to_string());
        let (head, response) = fixture.post(&good_headers(), body);
        assert!(status_line(&head).contains("400"), "{head}");
        assert!(!response.contains("raw"));
        fs::remove_file(fixture.root.join("ops.sock")).unwrap();

        let _rx =
            fixture.fake_ops(json!({"ok":false,"error":"internal","message":"boom"}).to_string());
        let (head, _) = fixture.post(&good_headers(), body);
        assert!(status_line(&head).contains("500"), "{head}");
        fs::remove_file(fixture.root.join("ops.sock")).unwrap();

        let _rx = fixture.fake_ops("garbage".into());
        let (head, _) = fixture.post(&good_headers(), body);
        assert!(status_line(&head).contains("503"), "{head}");
    }

    #[test]
    fn lists_filters_and_paginates_jobs() {
        let fixture = Fixture::new("list", Some(TOKEN));
        fixture.write_job(ID_A, "deploy", "succeeded", Some("demo"), 10);
        fixture.write_job(ID_B, "backup", "failed", Some("demo"), 20);
        fixture.write_job(ID_C, "host-collect", "running", None, 0);
        fs::write(
            fixture.jobs().join("20261021T000000Z-deadbeef.json"),
            "{broken",
        )
        .unwrap();
        fs::write(fixture.jobs().join("notes.json"), "{}").unwrap();
        fs::write(fixture.jobs().join(format!("{ID_A}.log")), "x").unwrap();

        let (head, body) = fixture.get("/api/jobs", "");
        assert!(status_line(&head).contains("200"), "{head}");
        let list = json_body(&body);
        let ids: Vec<_> = list["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|j| j["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [ID_C, ID_B, ID_A]);
        assert!(list["next_cursor"].is_null());

        let (_, body) = fixture.get("/api/jobs?limit=1", "");
        let page = json_body(&body);
        assert_eq!(page["jobs"][0]["id"], ID_C);
        assert_eq!(page["next_cursor"], ID_C);
        let (_, body) = fixture.get(&format!("/api/jobs?limit=1&cursor={ID_C}"), "");
        let page = json_body(&body);
        assert_eq!(page["jobs"][0]["id"], ID_B);
        let (_, body) = fixture.get(&format!("/api/jobs?limit=5&cursor={ID_B}"), "");
        assert_eq!(json_body(&body)["jobs"].as_array().unwrap().len(), 1);

        let (_, body) = fixture.get("/api/jobs?project=demo&state=failed", "");
        let list = json_body(&body);
        assert_eq!(list["jobs"].as_array().unwrap().len(), 1);
        assert_eq!(list["jobs"][0]["id"], ID_B);
        let (_, body) = fixture.get("/api/jobs?action=host-collect", "");
        assert_eq!(json_body(&body)["jobs"][0]["id"], ID_C);

        for bad in [
            "/api/jobs?limit=0",
            "/api/jobs?limit=201",
            "/api/jobs?limit=x",
            "/api/jobs?state=weird",
            "/api/jobs?action=weird",
            "/api/jobs?cursor=../../x",
            "/api/jobs?project=../x",
            "/api/jobs?other=1",
            "/api/jobs?limit=1&limit=2",
        ] {
            let (head, _) = fixture.get(bad, "");
            assert!(status_line(&head).contains("400"), "{bad}: {head}");
        }
    }

    #[test]
    fn gets_one_job_and_rejects_traversal() {
        let fixture = Fixture::new("get", Some(TOKEN));
        fixture.write_job(ID_A, "deploy", "succeeded", Some("demo"), 10);
        // Sanitization of a hostile record.
        fs::write(
            fixture.jobs().join(format!("{ID_B}.json")),
            json!({"id": ID_B, "action": "backup", "project": "../evil", "revision": "r\u{1b}[31m",
                "state": "failed", "actor": "bad actor!", "created_at_unix": 1, "started_at_unix": null,
                "finished_at_unix": 2, "exit_code": 1, "output_bytes": 0, "error": "line\nbreak", "token": "SECRET"})
            .to_string(),
        )
        .unwrap();
        let (head, body) = fixture.get(&format!("/api/jobs/{ID_A}"), "");
        assert!(status_line(&head).contains("200"));
        assert_eq!(json_body(&body)["state"], "succeeded");
        let (_, body) = fixture.get(&format!("/api/jobs/{ID_B}"), "");
        let job = json_body(&body);
        assert!(job["project"].is_null());
        assert_eq!(job["actor"], "unknown");
        assert_eq!(job["error"], "line break");
        assert_eq!(job["revision"], "r [31m");
        assert!(!body.contains("SECRET"));
        // A record whose id does not match its file name is ignored.
        fs::write(
            fixture.jobs().join(format!("{ID_C}.json")),
            fs::read(fixture.jobs().join(format!("{ID_A}.json"))).unwrap(),
        )
        .unwrap();
        for path in [
            "/api/jobs/20261030T000000Z-00000000".to_owned(),
            format!("/api/jobs/{ID_C}"),
            "/api/jobs/..%2F..%2Fprojects%2Fdemo".to_owned(),
            "/api/jobs/%2e%2e".to_owned(),
            format!("/api/jobs/{ID_A}.json"),
            format!("/api/jobs/{ID_A}%2F..%2F{ID_B}"),
            "/api/jobs/..%2Fdemo/output".to_owned(),
        ] {
            let (head, body) = fixture.get(&path, "");
            assert!(status_line(&head).contains("404"), "{path}: {head}");
            if !head.contains("text/plain") {
                assert_eq!(json_body(&body)["code"], "not_found");
            }
        }
        // A symlinked record is not followed.
        let outside = fixture.root.join("outside.json");
        fs::copy(fixture.jobs().join(format!("{ID_A}.json")), &outside).unwrap();
        let linked = "20261031T000000Z-11111111";
        std::os::unix::fs::symlink(&outside, fixture.jobs().join(format!("{linked}.json")))
            .unwrap();
        let (head, _) = fixture.get(&format!("/api/jobs/{linked}"), "");
        assert!(status_line(&head).contains("404"));
    }

    fn events(body: &str) -> Vec<(String, Option<u64>, Value)> {
        let mut out = Vec::new();
        for block in body.split("\n\n") {
            let (mut name, mut id, mut data) = (None, None, None);
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("event: ") {
                    name = Some(value.to_owned());
                } else if let Some(value) = line.strip_prefix("id: ") {
                    id = value.parse().ok();
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data = serde_json::from_str(value).ok();
                }
            }
            if let (Some(name), Some(data)) = (name, data) {
                out.push((name, id, data));
            }
        }
        out
    }

    #[test]
    fn streams_finished_output_and_resumes_from_offsets() {
        let fixture = Fixture::new("sse", Some(TOKEN));
        fixture.write_job(ID_A, "deploy", "succeeded", Some("demo"), 14);
        fs::write(
            fixture.jobs().join(format!("{ID_A}.log")),
            "héllo\nworld\nend",
        )
        .unwrap();

        let (head, body) = fixture.get(&format!("/api/jobs/{ID_A}/output"), "");
        assert!(status_line(&head).contains("200"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("content-type: text/event-stream"));
        assert!(lower.contains("cache-control: no-store"));
        assert!(lower.contains("x-accel-buffering: no"));
        let all = events(&body);
        assert_eq!(all.first().unwrap().0, "state");
        assert_eq!(all.last().unwrap().0, "end");
        let text: String = all
            .iter()
            .filter(|(name, ..)| name == "output")
            .map(|(.., data)| data["text"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(text, "héllo\nworld\nend");
        let last_output = all
            .iter()
            .rev()
            .find(|(name, ..)| name == "output")
            .unwrap();
        assert_eq!(last_output.1, Some(16));

        // Resume via Last-Event-ID (after the first line: 7 bytes because of the accent).
        let (_, body) = fixture.get(&format!("/api/jobs/{ID_A}/output"), "Last-Event-ID: 7\r\n");
        let resumed = events(&body);
        let first = resumed.iter().find(|(name, ..)| name == "output").unwrap();
        assert_eq!(first.2["offset"], 7);
        assert_eq!(first.2["text"], "world\nend");
        // Last-Event-ID wins over ?offset=.
        let (_, body) = fixture.get(
            &format!("/api/jobs/{ID_A}/output?offset=0"),
            "Last-Event-ID: 13\r\n",
        );
        let first = events(&body)
            .into_iter()
            .find(|(name, ..)| name == "output")
            .unwrap();
        assert_eq!(first.2["text"], "end");
        // ?offset= alone; beyond the end only state and end are sent.
        let (_, body) = fixture.get(&format!("/api/jobs/{ID_A}/output?offset=7"), "");
        let first = events(&body)
            .into_iter()
            .find(|(name, ..)| name == "output")
            .unwrap();
        assert_eq!(first.2["text"], "world\nend");
        let (_, body) = fixture.get(&format!("/api/jobs/{ID_A}/output?offset=9999"), "");
        let names: Vec<_> = events(&body).into_iter().map(|(name, ..)| name).collect();
        assert_eq!(names, ["state", "end"]);
        for bad in ["?offset=-1", "?offset=x", "?foo=1"] {
            let (head, _) = fixture.get(&format!("/api/jobs/{ID_A}/output{bad}"), "");
            assert!(status_line(&head).contains("400"), "{bad}: {head}");
        }
        let (head, _) = fixture.get(
            &format!("/api/jobs/{ID_A}/output"),
            "Last-Event-ID: abc\r\n",
        );
        assert!(status_line(&head).contains("400"));
        let (head, _) = fixture.get("/api/jobs/20260101T000000Z-00000000/output", "");
        assert!(status_line(&head).contains("404"));
    }

    #[test]
    fn follows_a_running_job_until_it_finishes() {
        let fixture = Fixture::new("follow", Some(TOKEN));
        fixture.write_job(ID_A, "backup", "running", Some("demo"), 0);
        let log = fixture.jobs().join(format!("{ID_A}.log"));
        fs::write(&log, "first\nsecond-par").unwrap();
        let writer_root = fixture.jobs();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(700));
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(writer_root.join(format!("{ID_A}.log")))
                .unwrap();
            file.write_all(b"t\nlast").unwrap();
            drop(file);
            thread::sleep(Duration::from_millis(300));
            let job = json!({"id": ID_A, "action": "backup", "project": "demo", "revision": null,
                "state": "succeeded", "actor": "corentin", "created_at_unix": 1, "started_at_unix": 2,
                "finished_at_unix": 3, "exit_code": 0, "output_bytes": 22, "error": null});
            fs::write(writer_root.join(format!("{ID_A}.json")), job.to_string()).unwrap();
        });
        let (_, body) = fixture.get(&format!("/api/jobs/{ID_A}/output"), "");
        writer.join().unwrap();
        let all = events(&body);
        let outputs: Vec<String> = all
            .iter()
            .filter(|(name, ..)| name == "output")
            .map(|(.., data)| data["text"].as_str().unwrap().to_owned())
            .collect();
        // The partial line is held back until it is complete, the tail is flushed at the end.
        assert_eq!(outputs, ["first\n", "second-part\n", "last"]);
        let names: Vec<_> = all.iter().map(|(name, ..)| name.as_str()).collect();
        assert_eq!(names.first(), Some(&"state"));
        assert_eq!(names.last(), Some(&"end"));
        assert_eq!(all.last().unwrap().2["state"], "succeeded");
    }

    #[test]
    fn logs_stream_validates_before_streaming() {
        let fixture = Fixture::new("logs-validate", Some(TOKEN));
        for bad in [
            "/api/logs/stream",
            "/api/logs/stream?project=missing",
            "/api/logs/stream?project=mon",
            "/api/logs/stream?project=demo&service=nope",
            "/api/logs/stream?project=demo&tail=0",
            "/api/logs/stream?project=demo&tail=1001",
            "/api/logs/stream?project=demo&since=;rm",
            "/api/logs/stream?project=demo&other=1",
            "/api/logs/stream?project=demo&tail=1&tail=2",
        ] {
            let (head, body) = fixture.get(bad, "");
            assert!(status_line(&head).contains("400"), "{bad}: {head}");
            assert_eq!(json_body(&body)["code"], "refused");
        }
        let (head, body) = fixture.get("/api/logs/stream?project=demo&tail=5", "");
        assert!(status_line(&head).contains("503"), "{head}");
        assert_eq!(json_body(&body)["code"], "executor_unavailable");
    }

    #[test]
    fn logs_stream_relays_frames_as_events() {
        let fixture = Fixture::new("logs-relay", Some(TOKEN));
        let received = fixture.fake_ops_on(
            "read.sock",
            &[
                r#"{"line":"hello"}"#.to_owned(),
                r#"{"line":"wörld \"q\""}"#.to_owned(),
                r#"{"end":"closed"}"#.to_owned(),
            ],
        );
        let (head, body) = fixture.get(
            "/api/logs/stream?project=demo&service=api&tail=7&since=10m",
            "",
        );
        assert!(status_line(&head).contains("200"), "{head}");
        assert!(head.to_ascii_lowercase().contains("text/event-stream"));
        let all = events(&body);
        let names: Vec<_> = all.iter().map(|(name, ..)| name.as_str()).collect();
        assert_eq!(names, ["line", "line", "end"]);
        assert_eq!(all[1].2["text"], "wörld \"q\"");
        assert_eq!(all[2].2["reason"], "closed");
        assert!(all.iter().all(|(_, id, _)| id.is_none()));
        let sent: Value = serde_json::from_str(&received.recv().unwrap()).unwrap();
        assert_eq!(
            sent,
            json!({"op":"logs-follow","project":"demo","service":"api","tail":7,"since":"10m"})
        );
    }

    #[test]
    fn logs_stream_reports_error_on_garbage_and_truncated_lines() {
        let fixture = Fixture::new("logs-garbage", Some(TOKEN));
        let long = "x".repeat(LOG_TEXT_LIMIT + 100);
        let _rx = fixture.fake_ops_on(
            "read.sock",
            &[json!({"line": long}).to_string(), "not json".to_owned()],
        );
        let (_, body) = fixture.get("/api/logs/stream?project=demo", "");
        let all = events(&body);
        assert_eq!(all[0].2["text"].as_str().unwrap().len(), LOG_TEXT_LIMIT);
        assert_eq!(all[1].0, "end");
        assert_eq!(all[1].2["reason"], "error");
    }

    #[test]
    fn closes_the_upstream_when_the_browser_leaves() {
        let fixture = Fixture::new("logs-leave", Some(TOKEN));
        let listener = UnixListener::bind(fixture.root.join("read.sock")).unwrap();
        let (tx, rx) = std_mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0u8; 512];
            let _ = stream.read(&mut buffer).unwrap();
            stream.write_all(b"{\"line\":\"one\"}\n").unwrap();
            // Keep writing until the peer is gone, then report it.
            let closed = (0..200).any(|_| {
                thread::sleep(Duration::from_millis(50));
                stream.write_all(b"{\"line\":\"more\"}\n").is_err()
            });
            tx.send(closed).unwrap();
        });
        let app = Arc::clone(&fixture.app);
        let address = {
            let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            std_listener.set_nonblocking(true).unwrap();
            let address = std_listener.local_addr().unwrap();
            thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async {
                    let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
                    axum::serve(listener, http::router(app)).await.unwrap();
                });
            });
            address
        };
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET /api/logs/stream?project=demo HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut seen = Vec::new();
        let mut buffer = [0u8; 1024];
        while !String::from_utf8_lossy(&seen).contains("one") {
            let count = client.read(&mut buffer).unwrap();
            assert!(count > 0);
            seen.extend_from_slice(&buffer[..count]);
        }
        drop(client);
        assert!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            "upstream stayed open"
        );
    }

    #[test]
    fn limits_concurrent_log_streams() {
        let counter = Arc::new(AtomicUsize::new(0));
        let slots: Vec<_> = (0..MAX_LOG_STREAMS)
            .map(|_| Slot::acquire(&counter, MAX_LOG_STREAMS).unwrap())
            .collect();
        assert!(Slot::acquire(&counter, MAX_LOG_STREAMS).is_none());
        drop(slots);
        assert!(Slot::acquire(&counter, MAX_LOG_STREAMS).is_some());
    }

    #[test]
    fn prune_plan_is_an_allowed_read() {
        let fixture = Fixture::new("plan", Some(TOKEN));
        let request =
            crate::read_request("/api/read?op=images-prune-plan", &fixture.app.config).unwrap();
        assert_eq!(request, json!({"op": "images-prune-plan"}));
        assert!(
            crate::read_request("/api/read?op=images-prune-plan&x=1", &fixture.app.config)
                .is_none()
        );
    }

    #[test]
    fn job_routes_reject_other_methods() {
        let fixture = Fixture::new("methods", Some(TOKEN));
        let (head, _) =
            fixture.send("DELETE /api/jobs HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(status_line(&head).contains("405"));
        let (head, _) =
            fixture.send("POST /api/jobs/x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(status_line(&head).contains("405"));
    }

    impl Fixture {
        /// Serves one connection on `<root>/<name>`, replying with `lines`, and
        /// yields the received request line.
        fn fake_ops_on(&self, name: &str, lines: &[String]) -> std_mpsc::Receiver<String> {
            let listener = UnixListener::bind(self.root.join(name)).unwrap();
            let (tx, rx) = std_mpsc::channel();
            let payload = lines.join("\n") + "\n";
            thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut received = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
                    received.push(byte[0]);
                }
                tx.send(String::from_utf8(received).unwrap()).unwrap();
                stream.write_all(payload.as_bytes()).unwrap();
            });
            rx
        }
    }
}
