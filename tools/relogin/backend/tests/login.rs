use anyhow::Result;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use sentinel_relogin::{
    accounts::{Account, AccountQuery, Accounts},
    api::{self, ApiState},
    config::Config,
    login::{
        protocol::{self, Action, Transport},
        LoginEngine, LoginOutput, Runner,
    },
    mail::{self, MailTicket, Mailbox},
    tasks::{Progress, Stage, Tasks},
    BoxFuture,
};
use sentinel_rs::flow::{
    auth,
    http::{HttpResponse, RequestSpec},
    sentinel::ClosureSource,
    store::{Account as StoredAccount, Store},
    SentinelToken,
};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tower::ServiceExt;

const SECRET: &str = "JBSWY3DPEHPK3PXP"; // Public synthetic RFC-style fixture, never an account secret.
const CALLBACK: &str =
    "https://chatgpt.com/api/auth/callback/openai?code=synthetic-code&state=synthetic-state";

fn response(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        body: body.into(),
        headers: vec![],
    }
}
fn redirect(location: &str) -> HttpResponse {
    HttpResponse {
        status: 302,
        headers: vec![("location".into(), location.into())],
        body: String::new(),
    }
}
fn continuation() -> HttpResponse {
    response(
        200,
        &serde_json::json!({"continue_url": CALLBACK}).to_string(),
    )
}

struct MockTransport {
    queue: Mutex<VecDeque<(&'static str, HttpResponse)>>,
    calls: Mutex<Vec<RequestSpec>>,
}
impl MockTransport {
    fn new(steps: Vec<(&'static str, HttpResponse)>) -> Self {
        Self {
            queue: Mutex::new(steps.into()),
            calls: Mutex::new(Vec::new()),
        }
    }
    fn assert_done(&self) {
        assert!(
            self.queue.lock().unwrap().is_empty(),
            "unconsumed protocol responses"
        );
        for spec in self.calls.lock().unwrap().iter() {
            assert!(
                !spec.url.contains("/oauth/token")
                    && !spec.url.contains("mfa_info")
                    && !spec.url.contains("user/register")
                    && !spec.url.contains("create_account")
            );
        }
    }
}
impl Transport for MockTransport {
    fn execute<'a>(&'a self, spec: &'a RequestSpec) -> BoxFuture<'a, Result<HttpResponse>> {
        Box::pin(async move {
            let (expected, reply) = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected HTTP call");
            assert_eq!(spec.label, expected);
            self.calls.lock().unwrap().push(spec.clone());
            Ok(reply)
        })
    }
    fn cookie_header_for(&self, url: &str) -> String {
        if url == auth::CHATGPT_BASE {
            let reads = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.url == auth::EP_CHAT_SESSION)
                .count();
            format!(
                "oai-did=synthetic-device; __Secure-next-auth.session-token=new-session-{reads}"
            )
        } else {
            "auth_session=new-auth-cookie; oai-did=synthetic-device".into()
        }
    }
}

#[derive(Default)]
struct MockMail {
    prepares: AtomicUsize,
    polls: AtomicUsize,
}
impl Mailbox for MockMail {
    fn prepare<'a>(&'a self, email: &'a str) -> BoxFuture<'a, Result<MailTicket>> {
        Box::pin(async move {
            assert_eq!(email, "demo@example.invalid");
            self.prepares.fetch_add(1, Ordering::SeqCst);
            Ok(MailTicket {
                url: "https://mail.example.invalid/demo".into(),
                baseline: HashSet::new(),
                issued_after: 0,
            })
        })
    }
    fn poll<'a>(&'a self, _: &'a MailTicket) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.polls.fetch_add(1, Ordering::SeqCst);
            Ok("123456".into())
        })
    }
}

fn bootstrap(landing: &str) -> Vec<(&'static str, HttpResponse)> {
    vec![
        (
            "chatgpt.warmup",
            response(200, r#"{"csrfToken":"synthetic-csrf"}"#),
        ),
        (
            "chatgpt.signin_openai",
            response(
                200,
                r#"{"url":"https://auth.openai.com/authorize?client_id=synthetic"}"#,
            ),
        ),
        ("auth.authorize_init", redirect(landing)),
        (
            "auth.authorize_init",
            response(200, "<html>synthetic login page</html>"),
        ),
    ]
}
fn finish_steps() -> Vec<(&'static str, HttpResponse)> {
    vec![
        ("auth.navigation", redirect("/")),
        (
            "auth.chat_session",
            response(200, r#"{"accessToken":"synthetic-initial-at"}"#),
        ),
    ]
}
fn account(secret: &str) -> Account {
    Account {
        id: 1,
        email: "demo@example.invalid".into(),
        password: "synthetic-password".into(),
        totp_secret: secret.into(),
    }
}

async fn run(
    mock: &MockTransport,
    mail: &dyn Mailbox,
    secret: &str,
) -> Result<(LoginOutput, Vec<Stage>)> {
    let stages = Arc::new(Mutex::new(Vec::new()));
    let progress_stages = stages.clone();
    let progress: Progress = Arc::new(move |stage| progress_stages.lock().unwrap().push(stage));
    let source = Arc::new(ClosureSource::new(|flow, did| {
        assert!([
            "authorize_continue",
            "password_verify",
            "email_otp_validate"
        ]
        .contains(&flow));
        assert_eq!(did, "synthetic-device");
        Ok(SentinelToken {
            token: "synthetic-sentinel".into(),
            so_token: None,
        })
    }));
    let engine = LoginEngine {
        transport: mock,
        source,
        mail,
        builder: auth::AuthRequestBuilder::new(
            sentinel_rs::flow::DEFAULT_USER_AGENT,
            "en-US,en;q=0.9",
            "",
        ),
    };
    let output = engine.run(&account(secret), progress).await?;
    let stages = stages.lock().unwrap().clone();
    Ok((output, stages))
}

#[tokio::test]
async fn password_login_does_not_touch_mail_mapping_or_enrollment() {
    let mut steps = bootstrap("https://auth.openai.com/log-in/password");
    steps.push(("auth.password_verify", continuation()));
    steps.extend(finish_steps());
    let mock = MockTransport::new(steps);
    let mail = MockMail::default();
    let (output, stages) = run(&mock, &mail, SECRET).await.unwrap();
    assert_eq!(output.access_token, "synthetic-initial-at");
    assert!(output.new_totp_secret.is_none());
    assert!(output.cookies.contains("new-session-1"));
    assert!(!stages.contains(&Stage::Enroll));
    assert_eq!(mail.prepares.load(Ordering::SeqCst), 0);
    let calls = mock.calls.lock().unwrap();
    assert!(
        calls[1].url.contains("screen_hint=login") && !calls[1].url.contains("login_or_signup")
    );
    assert_eq!(
        calls[4].body.as_ref().unwrap()["password"],
        "synthetic-password"
    );
    drop(calls);
    mock.assert_done();
}

#[tokio::test]
async fn existing_totp_challenge_uses_nested_factor_and_skips_enrollment() {
    let mut steps = bootstrap("https://auth.openai.com/log-in");
    steps.extend([
        ("auth.authorize_continue", response(200, r#"{"page":{"type":"login_password"}}"#)),
        ("auth.password_verify", response(200, r#"{"page":{"type":"mfa_challenge","payload":{"mfa_challenge_factors":[{"type":"sms","id":"ignore"},{"type":"totp","id":"factor-totp"}]}}}"#)),
        ("relogin.mfa_issue_challenge", response(200, "{}")),
        ("auth.mfa_verify", continuation()),
    ]);
    steps.extend(finish_steps());
    let mock = MockTransport::new(steps);
    let mail = MockMail::default();
    let (output, stages) = run(&mock, &mail, SECRET).await.unwrap();
    assert!(stages.contains(&Stage::Totp));
    assert!(output.new_totp_secret.is_none());
    let calls = mock.calls.lock().unwrap();
    let verify = calls.iter().find(|c| c.label == "auth.mfa_verify").unwrap();
    assert_eq!(verify.body.as_ref().unwrap()["id"], "factor-totp");
    assert_eq!(
        verify.body.as_ref().unwrap()["code"]
            .as_str()
            .unwrap()
            .len(),
        6
    );
    assert!(verify.header("referer").unwrap().ends_with("/factor-totp"));
    drop(calls);
    assert_eq!(mail.prepares.load(Ordering::SeqCst), 0);
    mock.assert_done();
}

#[tokio::test]
async fn explicit_email_challenge_uses_independent_mailbox_once() {
    let mut steps = bootstrap("https://auth.openai.com/log-in/password");
    steps.extend([
        (
            "auth.password_verify",
            response(200, r#"{"page":{"type":"email_otp_verification"}}"#),
        ),
        ("auth.email_otp_resend", response(200, "{}")),
        ("auth.email_otp_validate", continuation()),
    ]);
    steps.extend(finish_steps());
    let mock = MockTransport::new(steps);
    let mail = MockMail::default();
    let (_, stages) = run(&mock, &mail, SECRET).await.unwrap();
    assert!(stages.contains(&Stage::Email));
    assert_eq!(mail.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(mail.polls.load(Ordering::SeqCst), 1);
    let calls = mock.calls.lock().unwrap();
    let validate = calls
        .iter()
        .find(|c| c.label == "auth.email_otp_validate")
        .unwrap();
    assert_eq!(validate.body.as_ref().unwrap()["code"], "123456");
    assert!(!calls.iter().any(|c| c.url == auth::EP_OTP_SEND));
    drop(calls);
    mock.assert_done();
}

#[tokio::test]
async fn missing_local_secret_enrolls_activates_then_rereads_session() {
    let mut steps = bootstrap("https://auth.openai.com/log-in/password");
    steps.push(("auth.password_verify", continuation()));
    steps.extend(finish_steps());
    steps.extend([
        (
            "2fa.enroll",
            response(
                200,
                &format!(r#"{{"secret":"{SECRET}","session_id":"synthetic-enrollment"}}"#),
            ),
        ),
        ("2fa.activate", response(200, "{}")),
        (
            "auth.chat_session",
            response(200, r#"{"accessToken":"synthetic-final-at"}"#),
        ),
    ]);
    let mock = MockTransport::new(steps);
    let mail = MockMail::default();
    let (output, stages) = run(&mock, &mail, "").await.unwrap();
    assert_eq!(output.access_token, "synthetic-final-at");
    assert_eq!(output.new_totp_secret.as_deref(), Some(SECRET));
    assert!(output.cookies.contains("new-session-2"));
    assert!(stages.contains(&Stage::Enroll));
    assert_eq!(stages.iter().filter(|s| **s == Stage::Token).count(), 2);
    mock.assert_done();
}

#[tokio::test]
async fn activation_keeps_existing_completion_rule_without_a_recheck() {
    let mut steps = bootstrap("https://auth.openai.com/log-in/password");
    steps.push(("auth.password_verify", continuation()));
    steps.extend(finish_steps());
    steps.extend([
        (
            "2fa.enroll",
            response(
                200,
                &format!(r#"{{"secret":"{SECRET}","session_id":"synthetic-enrollment"}}"#),
            ),
        ),
        (
            "2fa.activate",
            response(
                400,
                r#"{"synthetic":"existing rule only checks request completion"}"#,
            ),
        ),
        (
            "auth.chat_session",
            response(200, r#"{"accessToken":"synthetic-final-at"}"#),
        ),
    ]);
    let mock = MockTransport::new(steps);
    assert!(run(&mock, &MockMail::default(), "")
        .await
        .unwrap()
        .0
        .new_totp_secret
        .is_some());
    mock.assert_done();
}

#[tokio::test]
async fn unknown_signup_page_stops_without_registration_or_mail() {
    let mock = MockTransport::new(bootstrap("https://auth.openai.com/create-account/password"));
    let mail = MockMail::default();
    assert!(run(&mock, &mail, SECRET).await.is_err());
    assert_eq!(mail.prepares.load(Ordering::SeqCst), 0);
    mock.assert_done();
}

#[tokio::test]
async fn missing_session_access_token_fails_before_enrollment() {
    let mut steps = bootstrap("https://auth.openai.com/log-in/password");
    steps.push(("auth.password_verify", continuation()));
    steps.extend([
        ("auth.navigation", redirect("/")),
        ("auth.chat_session", response(200, "{}")),
    ]);
    let mock = MockTransport::new(steps);
    assert!(run(&mock, &MockMail::default(), "").await.is_err());
    mock.assert_done();
}

#[test]
fn mapping_is_exact_and_reloaded_after_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("map.json");
    std::fs::write(
        &path,
        r#"{"demo@example.invalid":"https://mail.example.invalid/first"}"#,
    )
    .unwrap();
    assert!(mail::mapping::resolve(&path, "demo@example.invalid")
        .unwrap()
        .ends_with("first"));
    assert!(mail::mapping::resolve(&path, "DEMO@example.invalid").is_err());
    std::fs::write(
        &path,
        r#"{"demo@example.invalid":"https://mail.example.invalid/changed"}"#,
    )
    .unwrap();
    assert!(mail::mapping::resolve(&path, "demo@example.invalid")
        .unwrap()
        .ends_with("changed"));
}

#[test]
fn mail_reader_rejects_baseline_and_accepts_new_html_json_or_plain_codes() {
    let baseline = HashSet::from(["123456".into()]);
    assert!(mail::reader::fresh_code("123456", 0, &baseline).is_none());
    assert_eq!(
        mail::reader::fresh_code(r#"{"data":{"otp":"654321"}}"#, 0, &baseline).as_deref(),
        Some("654321")
    );
    assert_eq!(
        mail::reader::fresh_code(
            "<article><div class='code'>654321</div></article>",
            0,
            &baseline
        )
        .as_deref(),
        Some("654321")
    );
    assert!(mail::reader::fresh_code(
        "<article><div class='code'>123456</div></article>",
        0,
        &baseline
    )
    .is_none());
}

fn database() -> (tempfile::TempDir, Accounts, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("accounts.db");
    let store = Store::open(&path).unwrap();
    store.init().unwrap();
    for email in ["demo@example.invalid", "other@example.invalid"] {
        let mut account = StoredAccount::new(email, "synthetic-password");
        account.access_token = "synthetic-old-at".into();
        account.cookies = "old-cookie=value".into();
        account.totp_secret = SECRET.into();
        account.created_at = 42;
        account.updated_at = 43;
        store.save_account(&account).unwrap();
    }
    (dir, Accounts::new(&path), path)
}

#[test]
fn targeted_update_preserves_identity_password_created_time_and_other_account() {
    let (_dir, accounts, path) = database();
    accounts
        .save_login(
            1,
            &LoginOutput {
                access_token: "synthetic-new-at".into(),
                cookies: "new-cookie=value".into(),
                new_totp_secret: None,
            },
        )
        .unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    let row = conn.query_row("SELECT id,email,password,created_at,totp_secret,cookies,access_token,status FROM accounts WHERE id=1", [], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?))).unwrap();
    assert_eq!(
        (row.0, row.1.as_str(), row.2.as_str(), row.3, row.4.as_str()),
        (1, "demo@example.invalid", "synthetic-password", 42, SECRET)
    );
    assert_eq!(
        (row.5.as_str(), row.6.as_str(), row.7.as_str()),
        ("new-cookie=value", "synthetic-new-at", "registered")
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM accounts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        conn.query_row("SELECT access_token FROM accounts WHERE id=2", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "synthetic-old-at"
    );
    assert_eq!(
        conn.query_row("SELECT updated_at FROM accounts WHERE id=2", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        43
    );
    assert!(accounts
        .save_login(
            1,
            &LoginOutput {
                access_token: String::new(),
                cookies: String::new(),
                new_totp_secret: None
            }
        )
        .is_err());
    assert!(accounts
        .save_login(
            999,
            &LoginOutput {
                access_token: "synthetic".into(),
                cookies: "synthetic=1".into(),
                new_totp_secret: None
            }
        )
        .is_err());
}

#[test]
fn new_secret_updates_same_row_and_search_exposes_only_presence_flags() {
    let (_dir, accounts, _) = database();
    accounts
        .save_login(
            1,
            &LoginOutput {
                access_token: "synthetic-new-at".into(),
                cookies: "new=1".into(),
                new_totp_secret: Some("SYNTHETICNEWSECRET".into()),
            },
        )
        .unwrap();
    assert_eq!(
        accounts.get(1).unwrap().unwrap().totp_secret,
        "SYNTHETICNEWSECRET"
    );
    let page = accounts
        .list(&AccountQuery {
            q: Some("demo".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.total, 1);
    assert!(page.accounts[0].has_access_token);
    let json = serde_json::to_string(&page).unwrap();
    for secret in [
        "synthetic-password",
        "synthetic-new-at",
        "SYNTHETICNEWSECRET",
        "new=1",
    ] {
        assert!(!json.contains(secret));
    }
}

#[test]
fn config_is_independent_and_never_creates_missing_database() {
    let cfg = Config::from_lookup(|key| match key {
        "LOGIN_DB_PATH" => Some("missing.db".into()),
        "SENTINEL_PROXY" => Some("invalid".into()),
        _ => None,
    })
    .unwrap();
    assert!(cfg.proxy.is_none());
    assert!(Config::from_lookup(|key| match key {
        "LOGIN_DB_PATH" => Some("unused.db".into()),
        "LOGIN_LISTEN" => Some("0.0.0.0:8788".into()),
        _ => None,
    })
    .is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent.db");
    assert!(Accounts::new(&path).check().is_err());
    assert!(!path.exists());
}

struct ControlledRunner {
    gate: Arc<tokio::sync::Semaphore>,
    fail: bool,
}
impl Runner for ControlledRunner {
    fn run<'a>(&'a self, _: &'a Account, progress: Progress) -> BoxFuture<'a, Result<LoginOutput>> {
        Box::pin(async move {
            progress(Stage::Password);
            self.gate.acquire().await.unwrap().forget();
            if self.fail {
                anyhow::bail!("密码验证失败");
            }
            Ok(LoginOutput {
                access_token: "synthetic-new-at".into(),
                cookies: "new=1".into(),
                new_totp_secret: None,
            })
        })
    }
}

fn config(path: PathBuf) -> Config {
    Config::from_lookup(|key| (key == "LOGIN_DB_PATH").then(|| path.to_string_lossy().into_owned()))
        .unwrap()
}

async fn call(
    router: axum::Router,
    method: &str,
    path: &str,
    body: &str,
    token: bool,
    origin: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:8788")
        .header("content-type", "application/json");
    if token {
        request = request.header("authorization", format!("Bearer {}", "a".repeat(64)));
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = router
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}

async fn wait_done(tasks: &Tasks) {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if tasks.latest().unwrap().status != "running" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn api_single_task_gate_auth_and_same_row_update() {
    let (_dir, accounts, path) = database();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let tasks = Tasks::default();
    let router = api::router(ApiState {
        config: config(path.clone()),
        accounts: accounts.clone(),
        tasks: tasks.clone(),
        token: Arc::new("a".repeat(64)),
        runner: Arc::new(ControlledRunner {
            gate: gate.clone(),
            fail: false,
        }),
    });
    assert_eq!(
        call(router.clone(), "GET", "/api/accounts", "", false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            router.clone(),
            "POST",
            "/api/relogin",
            r#"{"account_id":1}"#,
            true,
            Some("https://example.invalid")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            router.clone(),
            "POST",
            "/api/relogin",
            r#"{"account_id":999}"#,
            true,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let first = call(
        router.clone(),
        "POST",
        "/api/relogin",
        r#"{"account_id":1}"#,
        true,
        None,
    )
    .await;
    assert_eq!(first.0, StatusCode::ACCEPTED);
    let second = call(
        router.clone(),
        "POST",
        "/api/relogin",
        r#"{"account_id":2}"#,
        true,
        None,
    )
    .await;
    assert_eq!(second.0, StatusCode::CONFLICT);
    assert_eq!(second.1["task"]["account_id"], 1);
    let current = call(
        router.clone(),
        "GET",
        "/api/relogin/current",
        "",
        true,
        None,
    )
    .await;
    assert_eq!(current.1["task"]["task_id"], first.1["task_id"]);
    gate.add_permits(1);
    wait_done(&tasks).await;
    assert_eq!(tasks.latest().unwrap().status, "succeeded");
    assert_eq!(
        accounts
            .list(&AccountQuery {
                account_id: Some(1),
                ..Default::default()
            })
            .unwrap()
            .accounts[0]
            .status,
        "registered"
    );
    let listing = call(router, "GET", "/api/accounts", "", true, None)
        .await
        .1
        .to_string();
    assert!(!listing.contains("synthetic-password") && !listing.contains("synthetic-new-at"));
}

#[tokio::test]
async fn failed_task_preserves_previous_database_credentials() {
    let (_dir, accounts, path) = database();
    let tasks = Tasks::default();
    tasks
        .start(
            accounts.get(1).unwrap().unwrap(),
            accounts,
            Arc::new(ControlledRunner {
                gate: Arc::new(tokio::sync::Semaphore::new(1)),
                fail: true,
            }),
        )
        .unwrap_or_else(|_| panic!("unexpected busy"));
    wait_done(&tasks).await;
    assert_eq!(tasks.latest().unwrap().status, "failed");
    let conn = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT access_token,cookies,updated_at FROM accounts WHERE id=1",
            [],
            |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?
            ))
        )
        .unwrap(),
        ("synthetic-old-at".into(), "old-cookie=value".into(), 43)
    );
}

#[test]
fn response_parser_and_error_messages_do_not_leak_remote_secrets() {
    let step = protocol::parse_step(
        r#"{"page":{"type":"mfa_challenge","url":"/mfa-challenge/factor-demo"}}"#,
    )
    .unwrap();
    assert_eq!(step.action, Action::Totp);
    assert_eq!(step.factor_id, "factor-demo");
    assert!(protocol::trusted_url(
        "https://example.invalid/api/auth/callback/openai?code=synthetic"
    )
    .is_err());
    let err = protocol::success(
        &response(
            401,
            r#"{"error":{"code":"incorrect_password","message":"synthetic-secret"}}"#,
        ),
        "密码验证",
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("密码不正确"));
    assert!(!err.contains("synthetic-secret"));
}

#[tokio::test]
async fn workspace_selection_consumes_callback_before_reading_session() {
    let mock = MockTransport::new(vec![
        ("auth.navigation", response(200, "<html>workspace</html>")),
        ("auth.workspace_select", continuation()),
        ("auth.navigation", redirect("/")),
        (
            "auth.chat_session",
            response(200, r#"{"accessToken":"synthetic-workspace-at"}"#),
        ),
    ]);
    let builder =
        auth::AuthRequestBuilder::new("synthetic-user-agent", "en-US", "synthetic-device");
    let token = protocol::finish(
        &mock,
        &builder,
        "https://auth.openai.com/workspace?workspace_id=synthetic-workspace",
    )
    .await
    .unwrap();
    assert_eq!(token, "synthetic-workspace-at");
    let calls = mock.calls.lock().unwrap();
    assert_eq!(
        calls[1].body.as_ref().unwrap()["workspace_id"],
        "synthetic-workspace"
    );
    assert_eq!(calls[2].url, CALLBACK);
    drop(calls);
    mock.assert_done();
}

#[tokio::test]
async fn independent_http_mail_reader_fetches_baseline_then_new_code() {
    use axum::{extract::State, http::HeaderMap, routing::get, Json, Router};
    let hits = Arc::new(AtomicUsize::new(0));
    async fn inbox(
        State(hits): State<Arc<AtomicUsize>>,
        headers: HeaderMap,
    ) -> Json<serde_json::Value> {
        assert_eq!(headers["authorization"], "Bearer synthetic-mail-token");
        let count = hits.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::json!({"otp": if count == 0 { "123456" } else { "654321" }}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/inbox", get(inbox))
        .with_state(hits.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let map = directory.path().join("map.json");
    std::fs::write(
        &map,
        serde_json::json!({"demo@example.invalid":format!("http://{address}/inbox")}).to_string(),
    )
    .unwrap();
    let mut config = config(directory.path().join("unused.db"));
    config.mail_map_file = map;
    config.mail_api_token = Some("synthetic-mail-token".into());
    config.proxy = Some("http://127.0.0.1:1".into()); // Authentication proxy must not affect email.
    let mailbox = mail::FileMailbox::new(&config).unwrap();
    let ticket = mailbox.prepare("demo@example.invalid").await.unwrap();
    assert!(ticket.baseline.contains("123456"));
    assert_eq!(mailbox.poll(&ticket).await.unwrap(), "654321");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    server.abort();
}
