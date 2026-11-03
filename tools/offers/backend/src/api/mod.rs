use crate::{
    application::{
        network,
        service::{blocking, Service},
    },
    domain::{
        campaign::Eligibility,
        model::{now, Summary},
    },
    infrastructure::{export, http::normalize_proxy_url, results::private_file},
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path as RoutePath, Query, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use rand::RngCore;
use serde::Deserialize;
use serde_json::json;
use std::{io::Write, path::Path, sync::Arc};

#[derive(Clone)]
pub struct ApiState {
    pub service: Arc<Service>,
    token: Arc<String>,
}

pub fn admin_token(path: &Path) -> anyhow::Result<String> {
    if !path.exists() {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        private_file(path, false)?.write_all(format!("{token}\n").as_bytes())?;
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > 256 {
        anyhow::bail!("invalid_admin_token_file")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("admin_token_requires_mode_0600")
        }
    }
    let token = std::fs::read_to_string(path)?.trim().to_owned();
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("invalid_admin_token_file")
    }
    Ok(token)
}

pub fn router(service: Arc<Service>, token: String) -> Router {
    let state = ApiState {
        service,
        token: Arc::new(token),
    };
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../../frontend/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    include_str!("../../../frontend/app.js"),
                )
            }),
        )
        .route(
            "/styles.css",
            get(|| async {
                (
                    [("content-type", "text/css; charset=utf-8")],
                    include_str!("../../../frontend/styles.css"),
                )
            }),
        )
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/accounts", get(accounts))
        .route("/api/accounts/{id}/offers", get(account))
        .route("/api/settings/proxy", get(proxy_settings).put(save_proxy))
        .route("/api/tasks", get(tasks).post(create_task))
        .route("/api/tasks/{id}", get(task))
        .route("/api/tasks/{id}/export", get(export_task))
        .layer(DefaultBodyLimit::max(65536))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error":code}))).into_response()
}

async fn guard(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    let port = state.service.config.listen.port();
    let valid_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host = req
        .headers()
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
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
            .unwrap_or("");
        let expected = state.token.as_bytes();
        let got = got.as_bytes();
        let difference = expected
            .iter()
            .enumerate()
            .fold(expected.len() ^ got.len(), |a, (i, b)| {
                a | usize::from(*b ^ got.get(i).copied().unwrap_or(0))
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
    headers.insert("content-security-policy",HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    response
}

type ApiResult = Result<Response, Response>;
fn internal(_: anyhow::Error) -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable")
}

async fn proxy_settings(State(state): State<ApiState>) -> ApiResult {
    let service = state.service;
    Ok(Json(
        blocking(move || network::settings(&service.config, &service.results))
            .await
            .map_err(internal)?,
    )
    .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveProxy {
    mode: String,
    proxy: Option<String>,
}

async fn save_proxy(State(state): State<ApiState>, Json(body): Json<SaveProxy>) -> ApiResult {
    let proxy = match (body.mode.as_str(), body.proxy) {
        ("proxy", Some(raw)) => Some(
            normalize_proxy_url(&raw)
                .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_proxy_url"))?,
        ),
        ("direct", None) => None,
        _ => return Err(error(StatusCode::BAD_REQUEST, "invalid_proxy_settings")),
    };
    let service = state.service;
    let settings = blocking(move || {
        service.results.set_query_proxy(proxy.as_deref())?;
        network::settings(&service.config, &service.results)
    })
    .await
    .map_err(internal)?;
    Ok(Json(settings).into_response())
}

#[derive(Default, Deserialize)]
struct AccountQuery {
    q: Option<String>,
    filter: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
}
async fn accounts(State(state): State<ApiState>, Query(query): Query<AccountQuery>) -> ApiResult {
    let filter = query.filter.as_deref().unwrap_or("all");
    if !["all", "free", "half", "unknown", "missing_token"].contains(&filter)
        || query.q.as_ref().is_some_and(|q| q.len() > 256)
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_filter"));
    }
    let page = query.page.unwrap_or(1);
    let size = query.page_size.unwrap_or(50);
    if page == 0 || size == 0 || size > 200 {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_pagination"));
    }
    let service = state.service.clone();
    let (views, cooldown) = blocking(move || {
        Ok((
            service.results.views(&service.source.accounts()?)?,
            service.results.cooldown()?,
        ))
    })
    .await
    .map_err(internal)?;
    let summary = Summary {
        total: views.len(),
        missing_token: views.iter().filter(|v| !v.account.has_token).count(),
        free_eligible: views
            .iter()
            .filter(|v| v.free_trial == Eligibility::Eligible)
            .count(),
        half_eligible: views
            .iter()
            .filter(|v| v.half_price == Eligibility::Eligible)
            .count(),
        pending: views
            .iter()
            .filter(|v| {
                v.free_trial == Eligibility::Unknown || v.half_price == Eligibility::Unknown
            })
            .count(),
    };
    let search = query.q.unwrap_or_default().to_lowercase();
    let selected: Vec<_> = views
        .into_iter()
        .filter(|v| v.account.email.to_lowercase().contains(&search))
        .filter(|v| match filter {
            "free" => v.free_trial == Eligibility::Eligible,
            "half" => v.half_price == Eligibility::Eligible,
            "unknown" => {
                v.free_trial == Eligibility::Unknown || v.half_price == Eligibility::Unknown
            }
            "missing_token" => !v.account.has_token,
            _ => true,
        })
        .collect();
    let total = selected.len();
    let items: Vec<_> = selected
        .into_iter()
        .skip(page.saturating_sub(1).saturating_mul(size))
        .take(size)
        .collect();
    Ok(Json(json!({"items":items,"total":total,"page":page,"page_size":size,"summary":summary,"cooldown_until":cooldown})).into_response())
}
async fn account(State(state): State<ApiState>, RoutePath(id): RoutePath<i64>) -> ApiResult {
    let service = state.service;
    let views = blocking(move || {
        let accounts = service
            .source
            .accounts()?
            .into_iter()
            .filter(|a| a.id == id)
            .collect::<Vec<_>>();
        service.results.views(&accounts)
    })
    .await
    .map_err(internal)?;
    views
        .into_iter()
        .next()
        .map(|v| Json(v).into_response())
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "account_not_found"))
}
async fn tasks(State(state): State<ApiState>) -> ApiResult {
    let result = state.service.results.clone();
    Ok(Json(blocking(move || result.tasks()).await.map_err(internal)?).into_response())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTask {
    scope: String,
    #[serde(default)]
    account_ids: Vec<i64>,
}
async fn create_task(State(state): State<ApiState>, Json(body): Json<CreateTask>) -> ApiResult {
    if !["all", "selected"].contains(&body.scope.as_str())
        || (body.scope == "all" && !body.account_ids.is_empty())
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_scope"));
    }
    match state
        .service
        .launch(body.scope == "all", body.account_ids)
        .await
    {
        Ok(id) => Ok((StatusCode::ACCEPTED, Json(json!({"task_id":id}))).into_response()),
        Err(e) => {
            let code = e.to_string();
            match code.as_str() {
                "task_busy" => Err(error(StatusCode::CONFLICT, &code)),
                "rate_limited" => {
                    let results = state.service.results.clone();
                    let until = blocking(move || results.cooldown())
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(now() + 60);
                    let mut response = error(StatusCode::TOO_MANY_REQUESTS, &code);
                    response.headers_mut().insert(
                        "retry-after",
                        HeaderValue::from_str(&(until - now()).max(1).to_string()).unwrap(),
                    );
                    Err(response)
                }
                "select_all_or_account_ids"
                | "invalid_account_ids"
                | "account_not_found"
                | "no_accounts_selected"
                | "invalid_proxy"
                | "invalid_proxy_url"
                | "empty_proxy" => Err(error(StatusCode::BAD_REQUEST, &code)),
                "service_stopping" => Err(error(StatusCode::SERVICE_UNAVAILABLE, &code)),
                _ => Err(internal(e)),
            }
        }
    }
}
async fn task(State(state): State<ApiState>, RoutePath(id): RoutePath<String>) -> ApiResult {
    state
        .service
        .task(&id)
        .await
        .map_err(internal)?
        .map(|t| Json(t).into_response())
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "task_not_found"))
}
#[derive(Deserialize)]
struct ExportQuery {
    format: Option<String>,
}
async fn export_task(
    State(state): State<ApiState>,
    RoutePath(id): RoutePath<String>,
    Query(query): Query<ExportQuery>,
) -> ApiResult {
    let format = query.format.unwrap_or_else(|| "csv".into());
    if !["csv", "json"].contains(&format.as_str()) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_export_format"));
    }
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(error(StatusCode::NOT_FOUND, "task_not_found"));
    }
    if state.service.task(&id).await.map_err(internal)?.is_none() {
        return Err(error(StatusCode::NOT_FOUND, "task_not_found"));
    }
    let result = state.service.results.clone();
    let key = id.clone();
    let kind = format.clone();
    let body = blocking(move || export::render(&result.checks(&key)?, &kind))
        .await
        .map_err(internal)?;
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        "content-type",
        HeaderValue::from_static(if format == "csv" {
            "text/csv; charset=utf-8"
        } else {
            "application/json; charset=utf-8"
        }),
    );
    response.headers_mut().insert(
        "content-disposition",
        HeaderValue::from_str(&format!("attachment; filename=offers-{id}.{format}")).unwrap(),
    );
    Ok(response)
}
