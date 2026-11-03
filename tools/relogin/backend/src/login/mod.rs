pub mod protocol;
pub mod twofa;

use crate::{
    accounts::Account,
    config::Config,
    mail::{FileMailbox, Mailbox},
    tasks::{Progress, Stage},
    BoxFuture,
};
use anyhow::{bail, Result};
use sentinel_rs::{
    fingerprint::DeviceFingerprint,
    flow::{auth, http, sentinel, SentinelToken, SentinelTokenSource},
};
use std::sync::Arc;

// Never serialized, logged or returned by the HTTP API.
pub struct LoginOutput {
    pub access_token: String,
    pub cookies: String,
    pub new_totp_secret: Option<String>,
}

pub trait Runner: Send + Sync {
    fn run<'a>(
        &'a self,
        account: &'a Account,
        progress: Progress,
    ) -> BoxFuture<'a, Result<LoginOutput>>;
}

pub struct LiveRunner {
    config: Config,
}

impl LiveRunner {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
}

impl Runner for LiveRunner {
    fn run<'a>(
        &'a self,
        account: &'a Account,
        progress: Progress,
    ) -> BoxFuture<'a, Result<LoginOutput>> {
        Box::pin(async move {
            progress(Stage::Session);
            let fp = DeviceFingerprint::generate(None);
            let http = http::Client::build_with_emulation(
                &fp.user_agent,
                self.config.proxy.as_deref(),
                http::RetryPolicy::default(),
                http::parse_emulation(&fp.emulation),
            )
            .map_err(|_| anyhow::anyhow!("无法创建账号登录客户端，请检查 LOGIN_PROXY"))?;
            let source = sentinel::live_source(
                &fp.user_agent,
                &fp.languages,
                self.config.proxy.as_deref(),
                Some(&fp),
            )
            .map_err(|_| anyhow::anyhow!("无法创建认证请求客户端"))?;
            let mail = FileMailbox::new(&self.config)?;
            let builder = auth::AuthRequestBuilder::new(&fp.user_agent, &fp.languages, "")
                .with_fingerprint(&fp);
            LoginEngine {
                transport: &http,
                source: Arc::new(source),
                mail: &mail,
                builder,
            }
            .run(account, progress)
            .await
        })
    }
}

pub struct LoginEngine<'a> {
    pub transport: &'a dyn protocol::Transport,
    pub source: Arc<dyn SentinelTokenSource>,
    pub mail: &'a dyn Mailbox,
    pub builder: auth::AuthRequestBuilder,
}

impl LoginEngine<'_> {
    pub async fn run(mut self, account: &Account, progress: Progress) -> Result<LoginOutput> {
        if account.password.trim().is_empty() {
            bail!("该账号未保存密码");
        }
        progress(Stage::Session);
        let (device_id, mut step) =
            protocol::initialize(self.transport, &self.builder, &account.email).await?;
        // Keep the signin logging ID while starting the landed document with the server device ID.
        let logging_id = self.builder.auth_session_logging_id().to_string();
        let ua = self.builder.user_agent().to_string();
        let lang = self.builder.accept_language().to_string();
        self.builder = auth::AuthRequestBuilder::new(&ua, &lang, &device_id);
        self.builder.set_auth_session_logging_id(logging_id);
        let mut last_sentinel = SentinelToken::default();
        if step.action == protocol::Action::Identify {
            last_sentinel = protocol::token(
                self.source.clone(),
                sentinel::FLOW_AUTHORIZE_CONTINUE,
                &device_id,
            )
            .await?;
            let spec = self.builder.authorize_continue_with(
                &account.email,
                &last_sentinel.token,
                auth::SCREEN_HINT_LOGIN,
                auth::PAGE_LOG_IN,
            );
            step = protocol::step_request(self.transport, &spec).await?;
        }
        let mut password_done = false;
        let mut email_done = false;
        let mut totp_done = false;
        let mut complete = false;
        for _ in 0..8 {
            match step.action {
                protocol::Action::Password if !password_done => {
                    progress(Stage::Password);
                    last_sentinel =
                        protocol::token(self.source.clone(), "password_verify", &device_id).await?;
                    step = protocol::step_request(
                        self.transport,
                        &self
                            .builder
                            .login_password_verify(&account.password, &last_sentinel),
                    )
                    .await?;
                    password_done = true;
                }
                protocol::Action::Email if !email_done => {
                    progress(Stage::Email);
                    let ticket = self.mail.prepare(&account.email).await?;
                    // Login resend keeps the current login challenge and its last Sentinel token.
                    // It never uses registration's send endpoint or email_otp flow.
                    let response =
                        protocol::request(self.transport, &self.builder.resend_otp(&last_sentinel))
                            .await?;
                    protocol::success(&response, "重发登录邮箱验证码")?;
                    let code = self.mail.poll(&ticket).await?;
                    let st = protocol::token(
                        self.source.clone(),
                        sentinel::FLOW_EMAIL_OTP_VALIDATE,
                        &device_id,
                    )
                    .await?;
                    step = protocol::step_request(
                        self.transport,
                        &self.builder.verify_otp(&code, &st),
                    )
                    .await?;
                    email_done = true;
                }
                protocol::Action::Totp if !totp_done => {
                    progress(Stage::Totp);
                    step = twofa::verify(
                        self.transport,
                        &self.builder,
                        self.source.clone(),
                        &device_id,
                        &account.totp_secret,
                        &step.factor_id,
                    )
                    .await?;
                    totp_done = true;
                }
                protocol::Action::Continue => {
                    complete = true;
                    break;
                }
                _ => bail!("登录返回未知或重复页面，当前任务已停止"),
            }
        }
        if !complete {
            bail!("登录挑战未完成");
        }
        progress(Stage::Token);
        let mut access_token =
            protocol::finish(self.transport, &self.builder, &step.continue_url).await?;
        let new_totp_secret = if account.totp_secret.trim().is_empty() {
            progress(Stage::Enroll);
            let secret = twofa::enroll(self.transport, &self.builder, &access_token).await?;
            progress(Stage::Token);
            access_token = protocol::session(self.transport, &self.builder).await?;
            Some(secret)
        } else {
            None
        };
        let cookies = protocol::cookies(self.transport);
        if cookies.trim().is_empty() {
            bail!("本次登录未导出 Cookie，账号未更新");
        }
        Ok(LoginOutput {
            access_token,
            cookies,
            new_totp_secret,
        })
    }
}
