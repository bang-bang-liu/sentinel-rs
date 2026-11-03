use anyhow::{bail, Context, Result};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Config {
    pub command: String,
    pub source_db: Option<PathBuf>,
    pub results_db: PathBuf,
    pub proxy: Option<String>,
    pub proxy_explicit: bool,
    pub emulation: Option<String>,
    pub listen: SocketAddr,
    pub admin_token_file: PathBuf,
    pub all: bool,
    pub ids: Vec<i64>,
    pub dry_run: bool,
    pub task_id: Option<String>,
    pub format: String,
    pub output: Option<PathBuf>,
    pub overwrite: bool,
    pub log: String,
}

pub const HELP:&str="sentinel-offers 0.1.1\n\nUsage:\n  sentinel-offers check --source-db PATH --results-db PATH (--all | --account-id ID...) [--dry-run]\n  sentinel-offers serve --source-db PATH --results-db PATH [--listen 127.0.0.1:8787]\n  sentinel-offers export --results-db PATH --task-id ID --format csv|json --output PATH\n\nOptions: --env-file PATH  --proxy URL  --direct  --admin-token-file PATH  --overwrite\nConfiguration: OFFERS_PROXY, OFFERS_HTTP_EMULATION, OFFERS_LOG.\nStarting the dashboard never starts a check. The source database is read-only.";

pub fn normalized(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute.exists() {
        return Ok(absolute.canonicalize()?);
    }
    let parent = absolute.parent().context("invalid_path")?;
    let name = absolute.file_name().context("invalid_path")?;
    Ok(normalized(parent)?.join(name))
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b || same_file::is_same_file(a, b).unwrap_or(false)
}

pub fn database_files(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        paths.push(PathBuf::from(p));
    }
    paths
}

impl Config {
    pub fn parse(args: Vec<String>) -> Result<Self> {
        let command = args.first().context("missing_command")?.clone();
        if !["check", "serve", "export"].contains(&command.as_str()) {
            bail!("unknown_command")
        }
        let mut values = HashMap::new();
        let mut ids = Vec::new();
        let mut flags = Vec::new();
        let mut it = args.into_iter().skip(1);
        while let Some(key) = it.next() {
            match key.as_str() {
                "--all" | "--dry-run" | "--direct" | "--overwrite" => {
                    if flags.contains(&key) {
                        bail!("duplicate_option: {key}")
                    }
                    flags.push(key);
                }
                "--account-id" => {
                    let id: i64 = it
                        .next()
                        .context("missing_account_id")?
                        .parse()
                        .context("invalid_account_id")?;
                    if id <= 0 {
                        bail!("invalid_account_id")
                    }
                    ids.push(id);
                }
                "--source-db" | "--results-db" | "--env-file" | "--proxy" | "--listen"
                | "--admin-token-file" | "--task-id" | "--format" | "--output" => {
                    let value = it.next().context("missing_option_value")?;
                    if values.insert(key.clone(), value).is_some() {
                        bail!("duplicate_option: {key}")
                    }
                }
                _ => bail!("unknown_option"),
            }
        }
        ids.sort_unstable();
        ids.dedup();
        if ids.len() > 10000 {
            bail!("too_many_accounts")
        }
        let has = |k: &str| flags.iter().any(|v| v == k);
        let all = has("--all");
        let dry_run = has("--dry-run");
        if command == "check" && (all == !ids.is_empty()) {
            bail!("select_all_or_account_ids")
        }
        if command != "check" && (all || !ids.is_empty() || dry_run) {
            bail!("option_not_valid_for_command")
        }
        if command != "export"
            && (has("--overwrite")
                || values.contains_key("--task-id")
                || values.contains_key("--output")
                || values.contains_key("--format"))
        {
            bail!("export_option_on_other_command")
        }
        if command != "serve"
            && (values.contains_key("--listen") || values.contains_key("--admin-token-file"))
        {
            bail!("serve_option_on_other_command")
        }
        if has("--direct") && values.contains_key("--proxy") {
            bail!("proxy_and_direct_conflict")
        }
        let mut file_env = HashMap::new();
        if let Some(path) = values.get("--env-file") {
            for item in dotenvy::from_path_iter(path).context("env_file_unreadable")? {
                let (key, value) = item.context("env_file_invalid")?;
                if key.starts_with("OFFERS_") {
                    file_env.insert(key, value);
                }
            }
        }
        let setting = |name: &str| {
            std::env::var(name)
                .ok()
                .or_else(|| file_env.get(name).cloned())
        };
        let proxy = if has("--direct") {
            None
        } else {
            values
                .get("--proxy")
                .cloned()
                .or_else(|| setting("OFFERS_PROXY"))
                .filter(|v| !v.trim().is_empty())
        };
        let source_db = values
            .get("--source-db")
            .map(|p| normalized(Path::new(p)))
            .transpose()?;
        if command != "export" && source_db.is_none() {
            bail!("source_db_required")
        }
        let results_db = normalized(Path::new(
            values.get("--results-db").context("results_db_required")?,
        ))?;
        let results_parent = results_db.parent().context("invalid_results_path")?;
        let admin_token_file = normalized(
            &values
                .get("--admin-token-file")
                .map(PathBuf::from)
                .unwrap_or_else(|| results_parent.join("admin.token")),
        )?;
        let output = values
            .get("--output")
            .map(|p| normalized(Path::new(p)))
            .transpose()?;
        let mut protected = database_files(&results_db);
        if let Some(source) = &source_db {
            protected.extend(database_files(source));
        }
        protected.extend([admin_token_file.clone(), results_parent.join("offers.lock")]);
        for i in 0..protected.len() {
            for j in i + 1..protected.len() {
                if same_file(&protected[i], &protected[j]) {
                    bail!("database_or_state_paths_overlap")
                }
            }
        }
        if let Some(source) = &source_db {
            if source.parent() == Some(results_parent)
                || source.parent() == admin_token_file.parent()
            {
                bail!("results_need_separate_directory")
            }
        }
        if let Some(path) = &output {
            if protected.iter().any(|v| same_file(path, v)) {
                bail!("output_overlaps_state_file")
            }
        }
        let listen: SocketAddr = values
            .get("--listen")
            .map(|s| s.as_str())
            .unwrap_or("127.0.0.1:8787")
            .parse()
            .context("invalid_listen_address")?;
        if !matches!(listen.ip(),IpAddr::V4(ip) if ip.is_loopback()) || listen.port() == 0 {
            bail!("listen_requires_ipv4_loopback")
        }
        let format = values
            .get("--format")
            .cloned()
            .unwrap_or_else(|| "csv".into());
        if !["csv", "json"].contains(&format.as_str()) {
            bail!("invalid_export_format")
        }
        let task_id = values.get("--task-id").cloned();
        if command == "export" && (task_id.is_none() || output.is_none()) {
            bail!("export_requires_task_id_and_output")
        }
        let emulation = setting("OFFERS_HTTP_EMULATION");
        if emulation
            .as_deref()
            .is_some_and(|v| crate::infrastructure::http::parse_emulation(v).is_none())
        {
            bail!("invalid_http_emulation")
        }
        let log = setting("OFFERS_LOG").unwrap_or_else(|| "info".into());
        if !["error", "warn", "info", "debug", "trace"].contains(&log.as_str()) {
            bail!("invalid_log_level")
        }
        Ok(Self {
            command,
            source_db,
            results_db,
            proxy,
            proxy_explicit: has("--direct") || values.contains_key("--proxy"),
            emulation,
            listen,
            admin_token_file,
            all,
            ids,
            dry_run,
            task_id,
            format,
            output,
            overwrite: has("--overwrite"),
            log,
        })
    }
}
