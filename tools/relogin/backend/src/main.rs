use anyhow::{bail, Context, Result};
use sentinel_relogin::{
    accounts::Accounts,
    api::{self, ApiState},
    config::Config,
    login::LiveRunner,
    tasks::Tasks,
};
use std::{path::PathBuf, sync::Arc};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut env_file: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "serve" => (),
            "--env-file" if env_file.is_none() => {
                env_file = Some(args.next().context("--env-file 需要文件路径")?.into());
            }
            "--version" | "-V" => {
                println!("sentinel-relogin {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--help" | "-h" => {
                println!("sentinel-relogin serve [--env-file PATH]\n只在界面点选账号后执行重登；配置使用独立 LOGIN_* 变量。");
                return Ok(());
            }
            _ => bail!("未知参数；使用 --help 查看用法"),
        }
    }
    // No implicit dotenv lookup. No registration configuration is loaded.
    if let Some(path) = env_file {
        dotenvy::from_path(path).context("无法读取指定的重登配置")?;
    }
    let config = Config::from_env()?;
    // Before runtime threads exist: make LOGIN_PROXY the only authentication proxy setting.
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "SENTINEL_LOG",
    ] {
        std::env::remove_var(name);
    }
    tracing_subscriber::fmt()
        .with_env_filter(format!("off,sentinel_relogin={}", config.log))
        .with_target(false)
        .init();
    let accounts = Accounts::new(config.db_path.clone());
    accounts
        .check()
        .context("无法读取已有账号库；不会自动创建或迁移数据库")?;
    let token = api::admin_token(&config.admin_token_file)?;
    let state = ApiState {
        accounts,
        tasks: Tasks::default(),
        runner: Arc::new(LiveRunner::new(config.clone())),
        token: Arc::new(token),
        config: config.clone(),
    };
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?.block_on(async move {
        let listener = tokio::net::TcpListener::bind(config.listen).await?;
        tracing::info!(listen = %config.listen, version = env!("CARGO_PKG_VERSION"), "sentinel-relogin ready");
        axum::serve(listener, api::router(state)).with_graceful_shutdown(shutdown()).await?;
        Ok(())
    })
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("signal handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
