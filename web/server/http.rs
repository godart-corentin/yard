//! HTTP surface of Yard Web, served by axum on a tokio runtime.
//!
//! The behavior is intentionally the one of the former hand-rolled server: fixed
//! routes, a fixed list of static files (no path-derived file access), `no-store`
//! API responses and the same security headers on every response.
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::{read_request, read_result, App, Config};

const JSON: &str = "application/json; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";
const CSP: &str = "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'";

type Shared = Arc<App>;

pub fn router(app: Shared) -> Router {
    Router::new()
        .route("/healthz", get(healthz).fallback(method_not_allowed))
        .route("/api/status", get(status).fallback(method_not_allowed))
        .route("/api/read", get(read).fallback(method_not_allowed))
        .route("/", static_route("index.html"))
        .route("/index.html", static_route("index.html"))
        .route("/styles.css", static_route("styles.css"))
        .route("/app.js", static_route("app.js"))
        .route("/favicon.svg", static_route("favicon.svg"))
        .fallback(fallback)
        .layer(middleware::from_fn(security_headers))
        .with_state(app)
}

pub fn serve(app: Shared) -> Result<(), String> {
    let address = format!("{}:{}", app.config.host, app.config.port);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start async runtime: {error}"))?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .map_err(|error| format!("cannot listen on {address}: {error}"))?;
        println!("Yard Web listening on {address}");
        axum::serve(listener, router(app))
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|error| format!("server error: {error}"))
    })
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let terminate = async {
        match signal(SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate => {}
    }
}

fn respond(status: StatusCode, content_type: &str, cache: &str, body: Vec<u8>) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (header::CACHE_CONTROL, cache.to_owned()),
        ],
        body,
    )
        .into_response()
}

fn text(status: StatusCode, body: &'static str) -> Response {
    respond(status, TEXT, "no-store", body.as_bytes().to_vec())
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(CSP),
    );
    response
}

async fn method_not_allowed() -> Response {
    text(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed\n")
}

async fn fallback(method: Method) -> Response {
    if method == Method::GET {
        text(StatusCode::NOT_FOUND, "Not found\n")
    } else {
        method_not_allowed().await
    }
}

async fn healthz() -> Response {
    respond(
        StatusCode::OK,
        JSON,
        "no-store",
        b"{\"status\":\"ok\"}\n".to_vec(),
    )
}

async fn status(State(app): State<Shared>) -> Response {
    // Status collection performs blocking HTTP checks and file reads.
    let body = tokio::task::spawn_blocking(move || serde_json::to_vec(&app.status())).await;
    match body {
        Ok(Ok(mut body)) => {
            body.push(b'\n');
            respond(StatusCode::OK, JSON, "no-store", body)
        }
        _ => text(StatusCode::INTERNAL_SERVER_ERROR, "Internal error\n"),
    }
}

async fn read(State(app): State<Shared>, uri: Uri) -> Response {
    let target = uri
        .path_and_query()
        .map(|value| value.as_str().to_owned())
        .unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || {
        read_request(&target, &app.config)
            .ok_or(400)
            .and_then(|request| read_result(&app.config.read_socket, &request))
    })
    .await
    .unwrap_or(Err(502));
    let (status, body) = match result {
        Ok(value) => (StatusCode::OK, value),
        Err(400) => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({"error": "Read operation refused"}),
        ),
        Err(503) => (
            StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({"error": "Read executor unavailable"}),
        ),
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            serde_json::json!({"error": "Read operation failed or output limit exceeded"}),
        ),
    };
    respond(
        status,
        JSON,
        "no-store",
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}

// Only fixed file names are ever joined to the static directory: no request
// data reaches the filesystem path.
fn static_route(filename: &'static str) -> axum::routing::MethodRouter<Shared> {
    get(move |State(app): State<Shared>| static_file(app, filename)).fallback(method_not_allowed)
}

async fn static_file(app: Shared, filename: &'static str) -> Response {
    let Ok(body) = tokio::fs::read(app.config.static_dir.join(filename)).await else {
        return text(StatusCode::NOT_FOUND, "Not found\n");
    };
    let content_type = match Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
    {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    respond(StatusCode::OK, content_type, "no-cache", body)
}

/// Container health probe (`yard-web --healthcheck`); needs no runtime.
pub fn healthcheck(config: &Config) -> Result<(), String> {
    let address = format!("127.0.0.1:{}", config.port);
    let mut stream = TcpStream::connect_timeout(
        &address
            .parse()
            .map_err(|error| format!("invalid address: {error}"))?,
        Duration::from_secs(2),
    )
    .map_err(|error| format!("cannot reach yard-web: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .map_err(|error| error.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| error.to_string())?;
    if response.starts_with("HTTP/1.1 200 ") {
        Ok(())
    } else {
        Err("yard-web health check returned a non-success response".to_owned())
    }
}

/// Test helper: serve one connection of the real router and return the raw response.
#[cfg(test)]
pub fn exchange(app: &Shared, raw_request: &str) -> String {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let address = std_listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let app = Arc::clone(app);
    let server = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
            axum::serve(listener, router(app))
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
    });
    let mut client = TcpStream::connect(address).unwrap();
    client.write_all(raw_request.as_bytes()).unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    let _ = stop.send(());
    server.join().unwrap();
    response
}
