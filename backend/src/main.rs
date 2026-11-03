use anyhow::{bail, Context, Result};
use sentinel_offers::{
    api,
    application::{
        config::{database_files, same_file, Config, HELP},
        service::Service,
    },
    infrastructure::{
        export,
        results::{private_file, Results},
        source::Source,
    },
};
use std::{io::Write, path::Path};
use tracing_subscriber::prelude::*;

fn main() {
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|v| v == "--help" || v == "-h") {
        println!("{HELP}");
        return;
    }
    if args == ["--version"] {
        println!("sentinel-offers {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let result = (|| -> Result<u8> {
        let config = Config::parse(args)?;
        let level = config
            .log
            .parse::<tracing::Level>()
            .unwrap_or(tracing::Level::INFO);
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(false)
                    .with_filter(
                        tracing_subscriber::filter::Targets::new()
                            .with_target("sentinel_offers", level),
                    ),
            )
            .init();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?
            .block_on(run(config))
    })();
    match result {
        Ok(code) => std::process::exit(i32::from(code)),
        Err(e) => {
            eprintln!("sentinel-offers: {e}");
            std::process::exit(1)
        }
    }
}

async fn run(config: Config) -> Result<u8> {
    if config.command == "export" {
        let result = Results::reader(&config.results_db)?;
        let output = config.output.as_ref().unwrap();
        let source = result.source_key()?;
        if database_files(Path::new(&source))
            .iter()
            .any(|p| same_file(output, p))
        {
            bail!("output_overlaps_source_file")
        }
        let id = config.task_id.as_ref().unwrap();
        if result.task(id)?.is_none() {
            bail!("task_not_found")
        }
        let body = export::render(&result.checks(id)?, &config.format)?;
        private_file(output, config.overwrite)?.write_all(body.as_bytes())?;
        println!("exported {}", output.display());
        return Ok(0);
    }
    let source = Source::open(config.source_db.as_ref().unwrap())?;
    if config.dry_run {
        let accounts = source.accounts()?;
        if !config.all
            && config
                .ids
                .iter()
                .any(|id| !accounts.iter().any(|a| a.id == *id))
        {
            bail!("account_not_found")
        }
        let accounts: Vec<_> = accounts
            .into_iter()
            .filter(|a| config.all || config.ids.contains(&a.id))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"dry_run":true,"accounts":accounts,"coupons":sentinel_offers::domain::campaign::COUPONS})
            )?
        );
        return Ok(0);
    }
    let (results, _lock) = Results::writer(&config.results_db, &source.key())?;
    // Saved dashboard settings take precedence over startup defaults.
    let (effective, _) = sentinel_offers::application::network::snapshot(&config, &results)?;
    sentinel_offers::application::checker::client(&effective)?;
    let service = Service::new(config.clone(), source, results);
    if config.command == "serve" {
        let token = api::admin_token(&config.admin_token_file)
            .context("admin_token_initialization_failed")?;
        let app = api::router(service.clone(), token);
        let listener = tokio::net::TcpListener::bind(config.listen)
            .await
            .context("dashboard_listen_failed")?;
        tracing::info!(listen=%config.listen,"dashboard_ready");
        let shutdown = service.clone();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                signal().await;
                shutdown.shutdown();
                shutdown.wait_idle().await;
            })
            .await?;
        return Ok(0);
    }
    let id = service.launch(config.all, config.ids.clone()).await?;
    println!("task_id={id}");
    let task = loop {
        if let Some(task) = service.task(&id).await? {
            if !matches!(task.status.as_str(), "queued" | "running") {
                break task;
            }
        }
        tokio::select! {_=signal()=>{service.shutdown();service.wait_idle().await;},_=tokio::time::sleep(std::time::Duration::from_millis(200))=>{}}
    };
    println!("{}", serde_json::to_string_pretty(&task)?);
    Ok(if task.status == "failed" {
        1
    } else if task.status != "completed" || task.unknown_checks > 0 {
        2
    } else {
        0
    })
}

async fn signal() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}};
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
