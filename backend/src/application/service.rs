use super::{checker, config::Config};
use crate::{
    domain::{
        campaign::{valid_token, Outcome, COUPONS},
        model::{Account, Task},
    },
    infrastructure::{results::Results, source::Source},
};
use anyhow::{bail, Result};
use std::sync::Arc;
use tokio::sync::{watch, Notify};

pub async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(f).await?
}

pub struct Service {
    pub config: Config,
    pub source: Source,
    pub results: Results,
    stop: watch::Sender<bool>,
    pub finished: Notify,
}

impl Service {
    pub fn new(config: Config, source: Source, results: Results) -> Arc<Self> {
        let (stop, _) = watch::channel(false);
        Arc::new(Self {
            config,
            source,
            results,
            stop,
            finished: Notify::new(),
        })
    }
    pub fn shutdown(&self) {
        let _ = self.stop.send_replace(true);
    }
    pub fn stopping(&self) -> bool {
        *self.stop.borrow()
    }
    pub async fn launch(self: &Arc<Self>, all: bool, ids: Vec<i64>) -> Result<String> {
        if self.stopping() {
            bail!("service_stopping")
        }
        if all == !ids.is_empty() {
            bail!("select_all_or_account_ids")
        }
        if ids.len() > 10000 || ids.iter().any(|v| *v <= 0) {
            bail!("invalid_account_ids")
        }
        let source = self.source.clone();
        let accounts = blocking(move || source.accounts()).await?;
        let accounts: Vec<_> = if all {
            accounts
        } else {
            if ids.iter().any(|id| !accounts.iter().any(|a| a.id == *id)) {
                bail!("account_not_found")
            }
            accounts
                .into_iter()
                .filter(|a| ids.contains(&a.id))
                .collect()
        };
        if accounts.is_empty() {
            bail!("no_accounts_selected")
        }
        let results = self.results.clone();
        let key = self.source.key();
        let selected = accounts.clone();
        let id = blocking(move || results.create_task(&key, &selected)).await?;
        let service = self.clone();
        let task_id = id.clone();
        tokio::spawn(async move {
            if service.run(&task_id, accounts).await.is_err() {
                let result = service.results.clone();
                let task = task_id.clone();
                let _ = blocking(move || {
                    result.finish(&task, "failed", Some("task_internal_error"), None)
                })
                .await;
                tracing::error!(task_id=%task_id,"task_internal_error");
            }
            service.finished.notify_waiters();
        });
        Ok(id)
    }
    async fn run(&self, id: &str, accounts: Vec<Account>) -> Result<()> {
        let result = self.results.clone();
        let task = id.to_owned();
        blocking(move || result.start_task(&task)).await?;
        let mut cancel = self.stop.subscribe();
        for account in accounts {
            if self.stopping() {
                return self.end(id, "interrupted", Some("interrupted"), None).await;
            }
            let source = self.source.clone();
            let a = account.clone();
            let credential = blocking(move || source.credential(a.id, &a.email)).await?;
            let token = credential.map(|c| c.access_token);
            let skip = match &token {
                None => Some("account_missing"),
                Some(v) if v.is_empty() => Some("missing_token"),
                Some(v) if !valid_token(v) => Some("invalid_token_format"),
                _ => None,
            };
            let client = if skip.is_none() {
                Some(checker::client(&self.config)?)
            } else {
                None
            };
            let mut auth_failed = false;
            for coupon in COUPONS {
                if self.stopping() {
                    return self.end(id, "interrupted", Some("interrupted"), None).await;
                }
                let outcome = if let Some(reason) = skip {
                    Outcome::unknown(reason, None)
                } else if auth_failed {
                    Outcome::unknown("auth_required", Some(401))
                } else {
                    let result = self.results.clone();
                    let task = id.to_owned();
                    let account_id = account.id;
                    blocking(move || result.start_check(&task, account_id, coupon)).await?;
                    let probe = tokio::select! {
                        _=cancel.changed()=>{return self.end(id,"interrupted",Some("interrupted"),None).await},
                        probe=checker::probe(client.as_ref().unwrap(),token.as_deref().unwrap(),coupon)=>probe
                    };
                    if let Some(until) = probe.stop_until {
                        self.save(id, account.id, coupon, probe.outcome).await?;
                        return self
                            .end(id, "interrupted", Some("deferred_rate_limit"), Some(until))
                            .await;
                    }
                    auth_failed = probe.outcome.reason == "auth_required";
                    probe.outcome
                };
                let requested = outcome.attempts > 0;
                self.save(id, account.id, coupon, outcome).await?;
                if requested {
                    tokio::select! {_=cancel.changed()=>{return self.end(id,"interrupted",Some("interrupted"),None).await},_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
                }
            }
        }
        self.end(id, "completed", None, None).await
    }
    async fn save(
        &self,
        id: &str,
        account_id: i64,
        coupon: &'static str,
        outcome: Outcome,
    ) -> Result<()> {
        let r = self.results.clone();
        let id = id.to_owned();
        blocking(move || r.record(&id, account_id, coupon, &outcome)).await
    }
    async fn end(
        &self,
        id: &str,
        status: &'static str,
        reason: Option<&'static str>,
        retry: Option<i64>,
    ) -> Result<()> {
        let r = self.results.clone();
        let id = id.to_owned();
        blocking(move || r.finish(&id, status, reason, retry)).await
    }
    pub async fn task(&self, id: &str) -> Result<Option<Task>> {
        let r = self.results.clone();
        let id = id.to_owned();
        blocking(move || r.task(&id)).await
    }
    pub async fn wait_idle(&self) {
        for _ in 0..50 {
            let r = self.results.clone();
            let busy = blocking(move || {
                Ok(r.tasks()?
                    .iter()
                    .any(|t| matches!(t.status.as_str(), "queued" | "running")))
            })
            .await
            .unwrap_or(false);
            if !busy {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}
