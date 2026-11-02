use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use rusqlite::Connection;
use sentinel_offers::{
    api,
    application::{
        checker::{probe_with, retry_after},
        config::Config,
        service::Service,
    },
    domain::{
        campaign::{aggregate, classify, Eligibility::*, Outcome, COUPONS},
        model::now,
    },
    infrastructure::{
        export,
        http::{HttpResponse, RequestSpec},
        results::{Results, WriterLock},
        source::Source,
    },
};
use std::path::PathBuf;
use tempfile::TempDir;
use tower::ServiceExt;

struct Fixture {
    _root: TempDir,
    config: Config,
    source: Source,
    results: Results,
    _lock: WriterLock,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source_path = root.path().join("accounts.db");
        let db = Connection::open(&source_path).unwrap();
        db.execute_batch("PRAGMA journal_mode=WAL;CREATE TABLE accounts(id INTEGER PRIMARY KEY,email TEXT,status TEXT,access_token TEXT,password TEXT,cookies TEXT,totp_secret TEXT);INSERT INTO accounts VALUES(1,'first@example.invalid','registered','SECRET_ACCESS','SECRET_PASS','SECRET_COOKIE','SECRET_2FA'),(2,'=HYPERLINK(test)','pending_otp',NULL,'','','');").unwrap();
        let config = Config::parse(vec![
            "serve".into(),
            "--source-db".into(),
            source_path.to_string_lossy().into(),
            "--results-db".into(),
            root.path()
                .join("results/offers.db")
                .to_string_lossy()
                .into(),
        ])
        .unwrap();
        let source = Source::open(&source_path).unwrap();
        let (results, lock) = Results::writer(&config.results_db, &source.key()).unwrap();
        Self {
            _root: root,
            config,
            source,
            results,
            _lock: lock,
        }
    }
}

#[test]
fn classifications_do_not_turn_errors_into_ineligible() {
    for coupon in COUPONS {
        assert_eq!(
            classify(
                coupon,
                200,
                &format!(r#"{{"coupon":"{coupon}","state":"eligible"}}"#)
            )
            .eligibility,
            Eligible
        );
        assert_eq!(
            classify(
                coupon,
                200,
                &format!(r#"{{"coupon":"{coupon}","state":"not_eligible"}}"#)
            )
            .eligibility,
            NotEligible
        );
        for state in ["offline", "something-new", ""] {
            let o = classify(
                coupon,
                200,
                &format!(r#"{{"coupon":"{coupon}","state":"{state}"}}"#),
            );
            assert_eq!(o.eligibility, Unknown);
        }
        for code in [401, 403, 429, 500, 503, 302, 404] {
            assert_eq!(classify(coupon, code, "{}").eligibility, Unknown);
        }
        for body in [
            "<html>not JSON</html>",
            r#"{"coupon":"different","state":"eligible"}"#,
            r#"{"state":"eligible"}"#,
            "[]",
            "null",
        ] {
            assert_eq!(classify(coupon, 200, body).eligibility, Unknown);
        }
    }
    assert_eq!(aggregate([NotEligible, NotEligible, Unknown]), Unknown);
    assert_eq!(
        aggregate([NotEligible, NotEligible, NotEligible]),
        NotEligible
    );
    assert_eq!(aggregate([NotEligible, Eligible, Unknown]), Eligible);
    assert_eq!(aggregate([]), Unknown);
}

#[test]
fn source_is_readonly_and_sees_committed_wal() {
    let f = Fixture::new();
    let c = f.source.connection().unwrap();
    for sql in [
        "INSERT INTO accounts(id) VALUES(9)",
        "UPDATE accounts SET access_token='changed'",
        "CREATE TABLE hacked(x)",
    ] {
        assert!(c.execute_batch(sql).is_err());
    }
    assert_eq!(f.source.accounts().unwrap().len(), 2);
    assert!(!f.source.accounts().unwrap()[0].has_token);
    let writer = Connection::open(f.config.source_db.as_ref().unwrap()).unwrap();
    writer.execute("INSERT INTO accounts(id,email,status,access_token) VALUES(3,'third@example.invalid','registered','dummy')",[]).unwrap();
    assert_eq!(f.source.accounts().unwrap().len(), 3);
    let absent = f._root.path().join("missing.db");
    assert!(Source::open(&absent).is_err());
    assert!(!absent.exists());
    assert!(f
        .source
        .credential(1, "wrong@example.invalid")
        .unwrap()
        .is_none());
    assert_eq!(
        f.source
            .credential(1, "first@example.invalid")
            .unwrap()
            .unwrap()
            .access_token,
        "SECRET_ACCESS"
    );
    let bad = f._root.path().join("bad.db");
    Connection::open(&bad)
        .unwrap()
        .execute_batch("CREATE TABLE accounts(id INTEGER,email TEXT)")
        .unwrap();
    assert!(Source::open(&bad).is_err());
}

#[test]
fn isolation_history_locking_recovery_and_cooldown() {
    let f = Fixture::new();
    let accounts = f.source.accounts().unwrap();
    assert!(Results::writer(&f.config.results_db, &f.source.key()).is_err());
    let old = f.results.create_task(&f.source.key(), &accounts).unwrap();
    assert!(f
        .results
        .create_task(&f.source.key(), &accounts)
        .unwrap_err()
        .to_string()
        .contains("task_busy"));
    let mut yes = classify(
        COUPONS[0],
        200,
        &format!(r#"{{"coupon":"{}","state":"eligible"}}"#, COUPONS[0]),
    );
    yes.attempts = 1;
    f.results.record(&old, 1, COUPONS[0], &yes).unwrap();
    f.results.finish(&old, "completed", None, None).unwrap();
    let new = f.results.create_task(&f.source.key(), &accounts).unwrap();
    f.results
        .record(&new, 1, COUPONS[0], &Outcome::unknown("timeout", None))
        .unwrap();
    let view = f
        .results
        .views(&accounts)
        .unwrap()
        .into_iter()
        .find(|v| v.account.id == 1)
        .unwrap();
    assert_eq!(view.free_trial, Unknown);
    assert_eq!(view.last_confirmed[0].task_id, old);
    let Fixture {
        _root,
        config,
        source,
        results,
        _lock,
    } = f;
    drop(_lock);
    let (_reopened, _guard) = Results::writer(&config.results_db, &source.key()).unwrap();
    let task = results.task(&new).unwrap().unwrap();
    assert_eq!(task.status, "interrupted");
    assert_eq!(task.completed_checks, 8);
    let id = results.create_task(&source.key(), &accounts).unwrap();
    results
        .finish(
            &id,
            "interrupted",
            Some("deferred_rate_limit"),
            Some(now() + 100),
        )
        .unwrap();
    assert!(results
        .create_task(&source.key(), &accounts)
        .unwrap_err()
        .to_string()
        .contains("rate_limited"));
    drop(_guard);
    assert!(Results::writer(&config.results_db, "another-source").is_err());
}

#[test]
fn paths_cannot_overwrite_source_state_or_sidecars() {
    let f = Fixture::new();
    let source = f.config.source_db.as_ref().unwrap();
    let args = |result: PathBuf| {
        vec![
            "check".into(),
            "--source-db".into(),
            source.to_string_lossy().into(),
            "--results-db".into(),
            result.to_string_lossy().into(),
            "--all".into(),
        ]
    };
    assert!(Config::parse(args(source.clone())).is_err());
    assert!(Config::parse(args(PathBuf::from(format!("{}-wal", source.display())))).is_err());
    std::fs::create_dir_all(f._root.path().join("aliases")).unwrap();
    let alias = f._root.path().join("aliases/hardlink.db");
    std::fs::hard_link(source, &alias).unwrap();
    assert!(Config::parse(args(alias)).is_err());
    let mut config = args(f.config.results_db.clone());
    config.extend(["--account-id".into(), "1".into()]);
    assert!(Config::parse(config).is_err());
    let source_bytes = std::fs::read(source).unwrap();
    assert!(Results::writer(source, &f.source.key()).is_err());
    assert_eq!(std::fs::read(source).unwrap(), source_bytes);
}

#[test]
fn api_and_export_models_never_contain_credentials() {
    let f = Fixture::new();
    let accounts = f.source.accounts().unwrap();
    let id = f.results.create_task(&f.source.key(), &accounts).unwrap();
    f.results.finish(&id, "completed", None, None).unwrap();
    let views = serde_json::to_string(&f.results.views(&accounts).unwrap()).unwrap();
    for body in [
        views,
        export::render(&f.results.checks(&id).unwrap(), "json").unwrap(),
        export::render(&f.results.checks(&id).unwrap(), "csv").unwrap(),
    ] {
        for secret in [
            "SECRET_ACCESS",
            "SECRET_PASS",
            "SECRET_COOKIE",
            "SECRET_2FA",
            "access_token",
            "totp_secret",
        ] {
            assert!(!body.contains(secret));
        }
    }
    assert_eq!(export::csv_cell("=1+1"), "\"'=1+1\"");
    assert_eq!(export::csv_cell("  +sum(1)"), "\"'  +sum(1)\"");
    assert_eq!(export::csv_cell("a\"b,c"), "\"a\"\"b,c\"");
    let json = export::render(&f.results.checks(&id).unwrap(), "json").unwrap();
    assert!(json.contains('T') && json.contains("Z\""));
    let req = RequestSpec::get("test", "https://invalid/?secret=SECRET_ACCESS")
        .with_headers(vec![("Authorization".into(), "SECRET_ACCESS".into())]);
    assert!(!format!("{req:?}").contains("SECRET"));
    let resp = HttpResponse {
        status: 200,
        headers: vec![],
        body: "SECRET_ACCESS".into(),
    };
    assert!(!format!("{resp:?}").contains("SECRET"));
}

#[tokio::test(start_paused = true)]
async fn retry_budget_and_long_retry_after() {
    let mut count = 0;
    let result = probe_with(COUPONS[0], || {
        count += 1;
        std::future::ready(Ok(HttpResponse {
            status: 500,
            ..Default::default()
        }))
    })
    .await;
    assert_eq!(count, 3);
    assert_eq!(result.outcome.attempts, 3);
    assert_eq!(result.outcome.reason, "upstream_error");
    count = 0;
    let result = probe_with(COUPONS[0], || {
        count += 1;
        std::future::ready(Ok(HttpResponse {
            status: 429,
            headers: vec![("Retry-After".into(), "120".into())],
            body: String::new(),
        }))
    })
    .await;
    assert_eq!(count, 1);
    assert!(result.stop_until.unwrap() >= now() + 119);
    let mut tries = 0;
    let result = probe_with(COUPONS[0], || {
        tries += 1;
        std::future::ready(if tries == 1 {
            Err(anyhow::anyhow!("test network"))
        } else {
            Ok(HttpResponse {
                status: 401,
                ..Default::default()
            })
        })
    })
    .await;
    assert_eq!(tries, 2);
    assert_eq!(result.outcome.reason, "auth_required");
    let date = std::time::UNIX_EPOCH + std::time::Duration::from_secs(784111777);
    assert_eq!(
        retry_after(Some("Sun, 06 Nov 1994 08:49:37 GMT"), date),
        Some(0)
    );
    assert_eq!(retry_after(Some("5"), date), Some(5));
    assert_eq!(retry_after(Some("broken"), date), None);
}

#[tokio::test(start_paused = true)]
async fn timeout_is_bounded_and_retried() {
    let mut count = 0;
    let result = probe_with(COUPONS[0], || {
        count += 1;
        async {
            tokio::time::sleep(std::time::Duration::from_secs(100)).await;
            Ok(HttpResponse::default())
        }
    })
    .await;
    assert_eq!(count, 3);
    assert_eq!(result.outcome.reason, "timeout");
    assert_eq!(result.outcome.eligibility, Unknown);
}

#[tokio::test]
async fn api_auth_origin_filters_export_and_busy() {
    let f = Fixture::new();
    let service = Service::new(f.config.clone(), f.source.clone(), f.results.clone());
    let token = "a".repeat(64);
    let router = api::router(service, token.clone());
    let req = |path: &str, auth: bool| {
        let mut r = Request::builder()
            .uri(path)
            .header("Host", "127.0.0.1:8787");
        if auth {
            r = r.header("Authorization", format!("Bearer {token}"));
        }
        r.body(Body::empty()).unwrap()
    };
    assert_eq!(
        router
            .clone()
            .oneshot(req("/api/accounts", false))
            .await
            .unwrap()
            .status(),
        401
    );
    let mut request = req("/api/accounts", true);
    request
        .headers_mut()
        .insert("origin", "https://evil.invalid".parse().unwrap());
    assert_eq!(router.clone().oneshot(request).await.unwrap().status(), 403);
    let mut request = req("/api/accounts", true);
    request
        .headers_mut()
        .insert("host", "evil.invalid:8787".parse().unwrap());
    assert_eq!(router.clone().oneshot(request).await.unwrap().status(), 403);
    let response = router
        .clone()
        .oneshot(req("/api/accounts?filter=missing_token&page_size=1", true))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["id"], 2);
    assert_eq!(body["summary"]["total"], 2);
    assert_eq!(
        router
            .clone()
            .oneshot(req("/api/accounts?filter=bad", true))
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        router
            .clone()
            .oneshot(req("/api/accounts?page_size=201", true))
            .await
            .unwrap()
            .status(),
        400
    );
    let id = f
        .results
        .create_task(&f.source.key(), &f.source.accounts().unwrap())
        .unwrap();
    let post = Request::builder()
        .method("POST")
        .uri("/api/tasks")
        .header("Host", "127.0.0.1:8787")
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(r#"{"scope":"all"}"#))
        .unwrap();
    assert_eq!(router.clone().oneshot(post).await.unwrap().status(), 409);
    let response = router
        .clone()
        .oneshot(req(&format!("/api/tasks/{id}/export?format=csv"), true))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    assert!(!body.contains("SECRET"));
    assert!(body.contains("'=HYPERLINK"));
    assert!(router
        .oneshot(req("/", false))
        .await
        .unwrap()
        .headers()
        .contains_key("content-security-policy"));
}

#[tokio::test]
async fn missing_token_task_finishes_without_network() {
    let f = Fixture::new();
    let service = Service::new(f.config.clone(), f.source.clone(), f.results.clone());
    let id = service.launch(false, vec![2]).await.unwrap();
    service.wait_idle().await;
    let task = service.task(&id).await.unwrap().unwrap();
    assert_eq!(task.status, "completed");
    assert_eq!(task.unknown_checks, 4);
    for row in f.results.checks(&id).unwrap() {
        assert_eq!(row.reason.as_deref(), Some("missing_token"));
        assert_eq!(row.attempts, 0);
    }
}

#[test]
fn independent_environment_and_dry_run_create_no_state() {
    let f = Fixture::new();
    let result = f._root.path().join("dryrun/offers.db");
    let env_file = f._root.path().join("offers.env");
    std::fs::write(
        &env_file,
        "SENTINEL_LOG=invalid\nTURB_WREQ_EMULATION=invalid\nOFFERS_LOG=warn\nOFFERS_PROXY=\n",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sentinel-offers"))
        .args([
            "check",
            "--source-db",
            f.config.source_db.as_ref().unwrap().to_str().unwrap(),
            "--results-db",
            result.to_str().unwrap(),
            "--env-file",
            env_file.to_str().unwrap(),
            "--all",
            "--dry-run",
        ])
        .env_remove("OFFERS_LOG")
        .env_remove("OFFERS_PROXY")
        .env_remove("OFFERS_HTTP_EMULATION")
        .env("SENTINEL_LOG", "invalid")
        .env("TURB_WREQ_EMULATION", "invalid")
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!result.exists());
    assert!(!result.parent().unwrap().exists());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("first@example.invalid"));
    assert!(!text.contains("SECRET"));
}

#[tokio::test]
async fn separate_clients_keep_cookies_separate() {
    use sentinel_offers::infrastructure::http::{Client, DEFAULT_EMULATION};
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let mut seen = vec![];
        for index in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = vec![];
            let mut chunk = [0u8; 1024];
            while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
            }
            seen.push(String::from_utf8(bytes).unwrap().to_lowercase());
            let cookie = if index == 0 {
                "Set-Cookie: isolation=client-one; Path=/\r\n"
            } else {
                ""
            };
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Type: application/json\r\nConnection: close\r\n{cookie}\r\n{{}}").unwrap();
        }
        seen
    });
    let first = Client::build(None, DEFAULT_EMULATION).unwrap();
    let second = Client::build(None, DEFAULT_EMULATION).unwrap();
    let spec = RequestSpec::get("test", format!("http://{address}/"));
    first.execute(&spec).await.unwrap();
    first.execute(&spec).await.unwrap();
    second.execute(&spec).await.unwrap();
    let seen = server.join().unwrap();
    assert!(!seen[0].contains("cookie:"));
    assert!(seen[1].contains("cookie: isolation=client-one"));
    assert!(!seen[2].contains("cookie:"));
    assert!(seen[0].contains("user-agent:"));
    assert!(!seen[0].contains("user-agent: \r\n"));
}
