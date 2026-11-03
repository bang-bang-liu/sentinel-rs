//! Local socket checks: mailbox requests bypass configured/system proxies;
//! registration requests continue through the explicitly configured proxy.
use sentinel_rs::flow::email::{EmailApiConfig, MailApiClient};
use sentinel_rs::flow::http::{Client, RequestSpec, RetryPolicy};
use sentinel_rs::flow::RegisterConfig;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn mailbox_requests_are_direct_while_registration_uses_proxy() {
    // Proxy environment changes stay inside a separate test process.
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "mail_routing_child", "--nocapture"])
        .env("SENTINEL_MAIL_ROUTING_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn serve(listener: TcpListener, requests: usize) -> thread::JoinHandle<Vec<String>> {
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut received = Vec::new();
        while received.len() < requests && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("local server: {error}"),
            };
            // Windows may inherit the listener's nonblocking mode on accept.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut data = Vec::new();
            let mut block = [0u8; 2048];
            while !data.windows(4).any(|part| part == b"\r\n\r\n") {
                let count = stream.read(&mut block).unwrap();
                if count == 0 { break; }
                data.extend_from_slice(&block[..count]);
                assert!(data.len() < 16_384);
            }
            let request = String::from_utf8(data).unwrap();
            let body = if request.starts_with("GET /code ") {
                "<article><div class=\"code\">123456</div></article>"
            } else {
                "{\"ok\":true}"
            };
            write!(stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body).unwrap();
            received.push(request);
        }
        received
    })
}

#[test]
fn mail_routing_child() {
    if std::env::var("SENTINEL_MAIL_ROUTING_TEST_CHILD").as_deref() != Ok("1") {
        return;
    }
    let mail_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mail_address = mail_listener.local_addr().unwrap();
    let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_url = format!("http://{}", proxy_listener.local_addr().unwrap());
    for key in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::set_var(key, &proxy_url);
    }
    for key in ["NO_PROXY", "no_proxy"] { std::env::set_var(key, ""); }
    let mail_server = serve(mail_listener, 2);
    let proxy_server = serve(proxy_listener, 1);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let cfg = RegisterConfig { proxy: Some(proxy_url.clone()), ..Default::default() };
        let mail_http = cfg.build_mail_http_client().unwrap();
        assert!(mail_http.proxy.is_none());
        let base = format!("http://{mail_address}");
        let api = EmailApiConfig { base_url: base.clone(), token: String::new(), allow_insecure: true };
        let mail = MailApiClient::new(&mail_http, api);
        assert_eq!(mail.health().await.unwrap()["ok"], true);
        assert!(mail.fetch_inbox(&format!("{base}/code")).await.unwrap().contains("123456"));

        let registration = Client::build("", cfg.proxy.as_deref(), RetryPolicy::default()).unwrap();
        let response = registration.execute(&RequestSpec::get("registration.routing", "http://registration.invalid/probe"))
            .await.unwrap();
        assert_eq!(response.status, 200);
    });
    let mail_requests = mail_server.join().unwrap();
    assert_eq!(mail_requests.len(), 2);
    assert!(mail_requests[0].starts_with("GET /health HTTP/1.1"));
    assert!(mail_requests[1].starts_with("GET /code HTTP/1.1"));
    let proxy_requests = proxy_server.join().unwrap();
    assert_eq!(proxy_requests.len(), 1);
    assert!(proxy_requests[0].starts_with("GET http://registration.invalid/probe HTTP/1.1"));
}
