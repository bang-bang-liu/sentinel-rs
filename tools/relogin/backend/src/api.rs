use crate::{
    accounts::{AccountQuery, Accounts},
    config::Config,
    login::Runner,
    tasks::Tasks,
};
use anyhow::{bail, Result};
use axum::{
    extract::{DefaultBodyLimit, Path as RoutePath, Query, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use std::{io::Write, path::Path, sync::Arc};

#[derive(Clone)]
pub struct ApiState {
    pub config: Config,
    pub accounts: Accounts,
    pub tasks: Tasks,
    pub runner: Arc<dyn Runner>,
    pub token: Arc<String>,
}

pub fn admin_token(path: &Path) -> Result<String> {
    if !path.exists() {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)?
            .write_all(format!("{token}\n").as_bytes())?;
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > 256 {
        bail!("invalid_admin_token_file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("admin_token_requires_mode_0600");
        }
    }
    let token = std::fs::read_to_string(path)?.trim().to_owned();
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid_admin_token_file");
    }
    Ok(token)
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../frontend/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../../frontend/app.js"),
                )
            }),
        )
        .route(
            "/styles.css",
            get(|| async {
                (
                    [("content-type", "text/css; charset=utf-8")],
                    include_str!("../../frontend/styles.css"),
                )
            }),
        )
        .route(
            "/health",
            get(|| async { Json(json!({"status":"ok","version":env!("CARGO_PKG_VERSION")})) }),
        )
        .route("/api/accounts", get(accounts))
        .route("/api/relogin", post(create_task))
        .route("/api/relogin/current", get(current_task))
        .route("/api/relogin/{task_id}", get(task))
        .layer(DefaultBodyLimit::max(4096))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}

async fn guard(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    let port = state.config.listen.port();
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let valid_hosts = [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("[::1]:{port}"),
    ];
    let mut response = if !valid_hosts.iter().any(|h| h == host) {
        error(StatusCode::FORBIDDEN, "invalid_host")
    } else if req
        .headers()
        .get("origin")
        .is_some_and(|v| v.to_str().ok() != Some(format!("http://{host}").as_str()))
        || req
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|v| v == "cross-site")
    {
        error(StatusCode::FORBIDDEN, "cross_origin_denied")
    } else if req.uri().path().starts_with("/api/") {
        let got = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("")
            .as_bytes();
        let expected = state.token.as_bytes();
        let difference = expected
            .iter()
            .enumerate()
            .fold(expected.len() ^ got.len(), |diff, (i, b)| {
                diff | usize::from(*b ^ got.get(i).copied().unwrap_or(0))
            });
        if difference != 0 {
            error(StatusCode::UNAUTHORIZED, "authentication_required")
        } else {
            next.run(req).await
        }
    } else {
        next.run(req).await
    };
    let headers = response.headers_mut();
    headers.insert("cache-control", HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}

async fn accounts(State(state): State<ApiState>, Query(query): Query<AccountQuery>) -> Response {
    if query.validate().is_err() {
        return error(StatusCode::BAD_REQUEST, "invalid_query");
    }
    match tokio::task::spawn_blocking(move || state.accounts.list(&query)).await {
        Ok(Ok(page)) => Json(page).into_response(),
        _ => error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartTask {
    account_id: i64,
}

async fn create_task(State(state): State<ApiState>, Json(body): Json<StartTask>) -> Response {
    if body.account_id <= 0 {
        return error(StatusCode::BAD_REQUEST, "invalid_account_id");
    }
    let accounts = state.accounts.clone();
    let account = match tokio::task::spawn_blocking(move || accounts.get(body.account_id)).await {
        Ok(Ok(Some(account))) => account,
        Ok(Ok(None)) => return error(StatusCode::NOT_FOUND, "account_not_found"),
        _ => return error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"),
    };
    if account.password.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "password_missing");
    }
    match state.tasks.start(account, state.accounts, state.runner) {
        Ok(id) => (StatusCode::ACCEPTED, Json(json!({"task_id":id}))).into_response(),
        Err(task) => (
            StatusCode::CONFLICT,
            Json(json!({"error":"task_busy","task":task})),
        )
            .into_response(),
    }
}

async fn current_task(State(state): State<ApiState>) -> Response {
    Json(json!({"task":state.tasks.latest()})).into_response()
}

async fn task(State(state): State<ApiState>, RoutePath(id): RoutePath<String>) -> Response {
    match state.tasks.get(&id) {
        Some(task) => Json(task).into_response(),
        None => error(StatusCode::NOT_FOUND, "task_not_found"),
    }
}
