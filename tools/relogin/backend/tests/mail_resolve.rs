use anyhow::Result;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use sentinel_relogin::{
    accounts::Account,
    config::Config,
    login::{protocol::Transport, LoginEngine},
    mail::{FileMailbox, Mailbox},
    BoxFuture,
};
use sentinel_rs::flow::{
    auth,
    http::{HttpResponse, RequestSpec},
    sentinel::ClosureSource,
    SentinelToken,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

const EMAIL: &str = "demo@example.invalid";
const CALLBACK: &str =
    "https://chatgpt.com/api/auth/callback/openai?code=synthetic&state=synthetic";

struct MailApi {
    origin: String,
    resolves: AtomicUsize,
    reads: AtomicUsize,
    resent: AtomicBool,
    events: Mutex<Vec<&'static str>>,
    status: StatusCode,
    body: Option<Value>,
}

/// 合成签发接口只接收当前邮箱及独立签发密钥。
async fn resolve(
    State(api): State<Arc<MailApi>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    assert_eq!(headers["authorization"], "Bearer synthetic-resolve-token");
    assert_eq!(headers["user-agent"], "sentinel-relogin/0.1.1");
    assert_eq!(body, json!({"email": EMAIL}));
    api.resolves.fetch_add(1, Ordering::SeqCst);
    api.events.lock().unwrap().push("resolve");
    (api.status, Json(api.body.clone().unwrap_or_else(|| json!({
        "email": EMAIL, "code_url": format!("{}/code?token=synthetic-link-token", api.origin),
        "expires_at": "2099-01-01T00:00:00.000Z"
    }))))
}

/// 模拟取码页，确认 API 模式不携带任何全局 Bearer 密钥。
async fn code(State(api): State<Arc<MailApi>>, headers: HeaderMap) -> Json<Value> {
    assert!(!headers.contains_key("authorization"));
    assert_eq!(headers["user-agent"], "sentinel-relogin/0.1.1");
    api.reads.fetch_add(1, Ordering::SeqCst);
    api.events.lock().unwrap().push("code");
    Json(json!({"otp": if api.resent.load(Ordering::SeqCst) { "654321" } else { "123456" }}))
}

struct Fixture {
    api: Arc<MailApi>,
    config: Config,
    server: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    /// 绑定随机回环端口，并让账号认证代理指向不可用端口来验证邮箱直连。
    async fn start(status: StatusCode, body: Option<Value>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let api = Arc::new(MailApi {
            origin: origin.clone(),
            resolves: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            resent: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
            status,
            body,
        });
        let router = Router::new()
            .route("/resolve", post(resolve))
            .route("/code", get(code))
            .with_state(api.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::from_lookup(|key| match key {
            "LOGIN_DB_PATH" => Some(
                directory
                    .path()
                    .join("unused.db")
                    .to_string_lossy()
                    .into_owned(),
            ),
            "LOGIN_MAIL_RESOLVE_URL" => Some(format!("{origin}/resolve")),
            "LOGIN_MAIL_RESOLVE_TOKEN" => Some("synthetic-resolve-token".into()),
            _ => None,
        })
        .unwrap();
        config.proxy = Some("http://127.0.0.1:1".into());
        config.mail_api_token = Some("legacy-token-must-not-be-sent".into());
        config.mail_map_file = directory.path().join("does-not-exist.json");
        Self {
            api,
            config,
            server,
            _directory: directory,
        }
    }
}

impl Drop for Fixture {
    /// 每项测试完成后停止本地模拟服务。
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn api_prepare_ignores_file_and_poll_reuses_one_signed_link() {
    let fixture = Fixture::start(StatusCode::CREATED, None).await;
    let mailbox = FileMailbox::new(&fixture.config).unwrap();
    let ticket = mailbox.prepare(EMAIL).await.unwrap();
    assert!(ticket.baseline.contains("123456"));
    fixture.api.resent.store(true, Ordering::SeqCst);
    assert_eq!(mailbox.poll(&ticket).await.unwrap(), "654321");
    assert_eq!(fixture.api.resolves.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 2);
    mailbox.prepare(EMAIL).await.unwrap();
    assert_eq!(fixture.api.resolves.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn resolve_http_errors_stop_without_reading_codes_or_falling_back_to_file() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::NOT_FOUND,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let fixture =
            Fixture::start(status, Some(json!({"error": "private-upstream-token"}))).await;
        std::fs::write(
            &fixture.config.mail_map_file,
            json!({EMAIL: format!("{}/code", fixture.api.origin)}).to_string(),
        )
        .unwrap();
        let error = FileMailbox::new(&fixture.config)
            .unwrap()
            .prepare(EMAIL)
            .await
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(&status.as_u16().to_string()));
        assert!(!error.contains("private-upstream-token"));
        assert!(!error.contains("synthetic-resolve-token"));
        assert_eq!(fixture.api.resolves.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn missing_resolve_token_is_checked_only_when_preparing_mail() {
    let mut fixture = Fixture::start(StatusCode::CREATED, None).await;
    fixture.config.mail_resolve_token = None;
    let mailbox = FileMailbox::new(&fixture.config).unwrap();
    assert_eq!(fixture.api.resolves.load(Ordering::SeqCst), 0);
    let error = mailbox.prepare(EMAIL).await.err().unwrap().to_string();
    assert!(error.contains("LOGIN_MAIL_RESOLVE_TOKEN"));
    assert_eq!(fixture.api.resolves.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_or_expired_responses_do_not_start_polling() {
    for body in [
        json!({"email": EMAIL}),
        json!({"email": "other@example.invalid", "code_url": "https://mail.example.invalid/code", "expires_at": "2099-01-01T00:00:00.000Z"}),
        json!({"email": EMAIL, "code_url": "https://mail.example.invalid/code", "expires_at": "2000-01-01T00:00:00.000Z"}),
        json!({"email": EMAIL, "code_url": "https://mail.example.invalid/code", "expires_at": "invalid"}),
        json!({"email": EMAIL, "code_url": "https://mail.example.invalid/code", "expires_at": "2099-01-01T00:00:00.000Z"}),
    ] {
        let fixture = Fixture::start(StatusCode::CREATED, Some(body)).await;
        assert!(FileMailbox::new(&fixture.config)
            .unwrap()
            .prepare(EMAIL)
            .await
            .is_err());
        assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 0);
    }
}

struct LoginTransport {
    responses: Mutex<VecDeque<(&'static str, HttpResponse)>>,
    api: Arc<MailApi>,
}

impl LoginTransport {
    /// 只提供本次模拟需要的登录响应，整个测试不访问真实认证站点。
    fn new(api: Arc<MailApi>, email_challenge: bool) -> Self {
        let response = |body: &str| HttpResponse {
            status: 200,
            body: body.into(),
            headers: vec![],
        };
        let redirect = |target: &str| HttpResponse {
            status: 302,
            body: "".into(),
            headers: vec![("location".into(), target.into())],
        };
        let continuation = json!({"continue_url": CALLBACK}).to_string();
        let mut replies = vec![
            (
                "chatgpt.warmup",
                response(r#"{"csrfToken":"synthetic-csrf"}"#),
            ),
            (
                "chatgpt.signin_openai",
                response(r#"{"url":"https://auth.openai.com/authorize?client_id=synthetic"}"#),
            ),
            (
                "auth.authorize_init",
                redirect("https://auth.openai.com/log-in/password"),
            ),
            (
                "auth.authorize_init",
                response("<html>synthetic login</html>"),
            ),
        ];
        if email_challenge {
            replies.extend([
                (
                    "auth.password_verify",
                    response(r#"{"page":{"type":"email_otp_verification"}}"#),
                ),
                ("auth.email_otp_resend", response("{}")),
                ("auth.email_otp_validate", response(&continuation)),
            ]);
        } else {
            replies.push(("auth.password_verify", response(&continuation)));
        }
        replies.extend([
            ("auth.navigation", redirect("/")),
            (
                "auth.chat_session",
                response(r#"{"accessToken":"synthetic-final-access"}"#),
            ),
        ]);
        Self {
            responses: Mutex::new(replies.into()),
            api,
        }
    }
}

impl Transport for LoginTransport {
    /// 把登录 resend 作为新邮件出现的触发点，验证真实模块间的调用顺序。
    fn execute<'a>(&'a self, request: &'a RequestSpec) -> BoxFuture<'a, Result<HttpResponse>> {
        Box::pin(async move {
            let (expected, response) = self.responses.lock().unwrap().pop_front().unwrap();
            assert_eq!(request.label, expected);
            assert!(
                !request.url.contains("/oauth/token")
                    && !request.url.contains("mfa_info")
                    && request.url != auth::EP_OTP_SEND
            );
            if request.label == "auth.email_otp_resend" {
                assert_eq!(self.api.resolves.load(Ordering::SeqCst), 1);
                assert_eq!(self.api.reads.load(Ordering::SeqCst), 1);
                self.api.events.lock().unwrap().push("resend");
                self.api.resent.store(true, Ordering::SeqCst);
            }
            if request.label == "auth.email_otp_validate" {
                assert_eq!(request.body.as_ref().unwrap()["code"], "654321");
                self.api.events.lock().unwrap().push("validate");
            }
            Ok(response)
        })
    }

    /// 返回合成 Cookie，以覆盖登录正常收尾。
    fn cookie_header_for(&self, _: &str) -> String {
        "oai-did=synthetic-device; synthetic-session=1".into()
    }
}

/// 使用真实远程邮箱适配器与模拟认证传输执行完整登录编排。
async fn run_login(fixture: &Fixture, email_challenge: bool) {
    let transport = LoginTransport::new(fixture.api.clone(), email_challenge);
    let mail = FileMailbox::new(&fixture.config).unwrap();
    let engine = LoginEngine {
        transport: &transport,
        mail: &mail,
        source: Arc::new(ClosureSource::new(|_, _| {
            Ok(SentinelToken {
                token: "synthetic-sentinel".into(),
                so_token: None,
            })
        })),
        builder: auth::AuthRequestBuilder::new(sentinel_rs::flow::DEFAULT_USER_AGENT, "en-US", ""),
    };
    let account = Account {
        id: 1,
        email: EMAIL.into(),
        password: "synthetic-password".into(),
        totp_secret: "JBSWY3DPEHPK3PXP".into(),
    };
    let output = engine.run(&account, Arc::new(|_| {})).await.unwrap();
    assert_eq!(output.access_token, "synthetic-final-access");
    assert!(transport.responses.lock().unwrap().is_empty());
}

#[tokio::test]
async fn login_resolves_baseline_then_resends_polls_and_submits() {
    let fixture = Fixture::start(StatusCode::CREATED, None).await;
    run_login(&fixture, true).await;
    assert_eq!(
        *fixture.api.events.lock().unwrap(),
        ["resolve", "code", "resend", "code", "validate"]
    );
}

#[tokio::test]
async fn login_without_email_challenge_makes_zero_mail_requests() {
    let mut fixture = Fixture::start(StatusCode::CREATED, None).await;
    fixture.config.mail_resolve_token = None;
    run_login(&fixture, false).await;
    assert_eq!(fixture.api.resolves.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.api.reads.load(Ordering::SeqCst), 0);
}
