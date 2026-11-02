use crate::domain::{
    campaign::{aggregate, Eligibility, Outcome, COUPONS},
    model::{now, Account, AccountView, Check, Task},
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Results {
    path: PathBuf,
}
pub struct WriterLock {
    _file: File,
}

pub fn private_file(path: &Path, overwrite: bool) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if overwrite {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    Ok(opts.open(path).context("output_file_unavailable")?)
}

impl Results {
    pub fn writer(path: &Path, source_key: &str) -> Result<(Self, WriterLock)> {
        let parent = path.parent().context("results_parent_missing")?;
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = opts.open(parent.join("offers.lock"))?;
        file.try_lock_exclusive().context("results_in_use")?;
        if !path.exists() {
            drop(private_file(path, false)?);
        }
        let this = Self {
            path: path.canonicalize()?,
        };
        let c = this.connect()?;
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 1 {
            bail!("results_schema_newer_than_program");
        }
        let tables: i64 = c.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if version == 0 && tables > 0 {
            bail!("results_db_not_empty")
        }
        if version == 1 {
            let saved: String = c
                .query_row(
                    "SELECT value FROM offer_meta WHERE key='source_key'",
                    [],
                    |r| r.get(0),
                )
                .context("results_schema_incompatible")?;
            if saved != source_key {
                bail!("results_bound_to_another_source")
            }
        }
        c.execute_batch(include_str!("../../migrations/001_offers.sql"))?;
        let saved: Option<String> = c
            .query_row(
                "SELECT value FROM offer_meta WHERE key='source_key'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if saved.as_deref().is_some_and(|s| s != source_key) {
            bail!("results_bound_to_another_source");
        }
        c.execute(
            "INSERT OR IGNORE INTO offer_meta(key,value) VALUES('source_key',?1)",
            [source_key],
        )?;
        c.execute("UPDATE offer_checks SET execution_status='finished',eligibility='unknown',reason='interrupted',checked_at=?1 WHERE execution_status!='finished'",[now()])?;
        c.execute("UPDATE offer_tasks SET status='interrupted',finished_at=?1,stop_reason='process_interrupted' WHERE status IN ('queued','running')",[now()])?;
        Ok((this, WriterLock { _file: file }))
    }
    pub fn reader(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.canonicalize().context("results_db_missing")?,
        })
    }
    pub fn source_key(&self) -> Result<String> {
        Ok(self.read()?.query_row(
            "SELECT value FROM offer_meta WHERE key='source_key'",
            [],
            |r| r.get(0),
        )?)
    }
    fn connect(&self) -> Result<Connection> {
        let c = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        c.busy_timeout(Duration::from_secs(3))?;
        c.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(c)
    }
    fn read(&self) -> Result<Connection> {
        let c = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        c.busy_timeout(Duration::from_secs(3))?;
        c.execute_batch("PRAGMA query_only=ON;")?;
        Ok(c)
    }
    pub fn cooldown(&self) -> Result<Option<i64>> {
        Ok(self.read()?.query_row(
            "SELECT max(retry_after_at) FROM offer_tasks WHERE retry_after_at>?1",
            [now()],
            |r| r.get(0),
        )?)
    }
    pub fn create_task(&self, source: &str, accounts: &[Account]) -> Result<String> {
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let busy: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM offer_tasks WHERE status IN ('queued','running'))",
            [],
            |r| r.get(0),
        )?;
        if busy {
            bail!("task_busy");
        }
        let cooldown: Option<i64> = tx.query_row(
            "SELECT max(retry_after_at) FROM offer_tasks WHERE retry_after_at>?1",
            [now()],
            |r| r.get(0),
        )?;
        if cooldown.is_some() {
            bail!("rate_limited");
        }
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO offer_tasks(task_id,source_key,status,created_at) VALUES(?1,?2,'queued',?3)",params![id,source,now()])?;
        for a in accounts {
            for coupon in COUPONS {
                tx.execute("INSERT INTO offer_checks(task_id,account_id,email,coupon,execution_status,attempts) VALUES(?1,?2,?3,?4,'pending',0)",params![id,a.id,a.email,coupon])?;
            }
        }
        tx.commit()?;
        Ok(id)
    }
    pub fn start_task(&self, id: &str) -> Result<()> {
        self.connect()?.execute(
            "UPDATE offer_tasks SET status='running',started_at=?2 WHERE task_id=?1",
            params![id, now()],
        )?;
        Ok(())
    }
    pub fn start_check(&self, id: &str, account_id: i64, coupon: &str) -> Result<()> {
        self.connect()?.execute("UPDATE offer_checks SET execution_status='running' WHERE task_id=?1 AND account_id=?2 AND coupon=?3",params![id,account_id,coupon])?;
        Ok(())
    }
    pub fn record(&self, id: &str, account_id: i64, coupon: &str, outcome: &Outcome) -> Result<()> {
        self.connect()?.execute("UPDATE offer_checks SET execution_status='finished',eligibility=?4,reason=?5,http_status=?6,upstream_state=?7,attempts=?8,checked_at=?9,redemption_status=?10 WHERE task_id=?1 AND account_id=?2 AND coupon=?3",
            params![id,account_id,coupon,outcome.eligibility.as_str(),outcome.reason,outcome.http_status,outcome.upstream_state,outcome.attempts,now(),outcome.redemption_status])?;
        Ok(())
    }
    pub fn finish(
        &self,
        id: &str,
        status: &str,
        reason: Option<&str>,
        retry_after: Option<i64>,
    ) -> Result<()> {
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        tx.execute("UPDATE offer_checks SET execution_status='finished',eligibility='unknown',reason=?2,checked_at=?3 WHERE task_id=?1 AND execution_status!='finished'",params![id,reason.unwrap_or("interrupted"),now()])?;
        tx.execute("UPDATE offer_tasks SET status=?2,finished_at=?3,stop_reason=?4,retry_after_at=?5 WHERE task_id=?1",params![id,status,now(),reason,retry_after])?;
        tx.commit()?;
        Ok(())
    }
    pub fn tasks(&self) -> Result<Vec<Task>> {
        let c = self.read()?;
        let mut q = c.prepare("SELECT task_id FROM offer_tasks ORDER BY rowid DESC LIMIT 50")?;
        let ids = q
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| task_from(&c, &id)?.context("task_not_found"))
            .collect()
    }
    pub fn task(&self, id: &str) -> Result<Option<Task>> {
        task_from(&self.read()?, id)
    }
    pub fn checks(&self, id: &str) -> Result<Vec<Check>> {
        checks_from(&self.read()?, id, None)
    }
    pub fn views(&self, accounts: &[Account]) -> Result<Vec<AccountView>> {
        let c = self.read()?;
        accounts.iter().map(|a| {
            let latest:Option<String>=c.query_row("SELECT c.task_id FROM offer_checks c JOIN offer_tasks t ON t.task_id=c.task_id WHERE c.account_id=?1 AND c.email=?2 ORDER BY t.rowid DESC LIMIT 1",params![a.id,a.email],|r|r.get(0)).optional()?;
            let checks=match &latest {Some(id)=>checks_from(&c,id,Some(a.id))?,None=>vec![]};
            let get=|coupon:&str|checks.iter().find(|r|r.coupon==coupon).and_then(|r|r.eligibility).unwrap_or(Eligibility::Unknown);
            let free_trial=get(COUPONS[0]);
            let half_price=aggregate(COUPONS[1..].iter().map(|coupon|get(coupon)));
            let mut confirmed=Vec::new();
            for coupon in COUPONS {
                let found=c.query_row("SELECT c.task_id,c.account_id,c.email,c.coupon,c.execution_status,c.eligibility,c.reason,c.http_status,c.upstream_state,c.attempts,c.checked_at,c.redemption_status FROM offer_checks c JOIN offer_tasks t ON t.task_id=c.task_id WHERE c.account_id=?1 AND c.email=?2 AND c.coupon=?3 AND c.eligibility IN ('eligible','not_eligible') ORDER BY t.rowid DESC LIMIT 1",params![a.id,a.email,coupon],check_row).optional()?;
                if let Some(row)=found {confirmed.push(row)}
            }
            Ok(AccountView{account:a.clone(),free_trial,half_price,checked_at:checks.iter().filter_map(|r|r.checked_at).max(),latest_task_id:latest,checks,last_confirmed:confirmed})
        }).collect()
    }
}

fn task_from(c: &Connection, id: &str) -> Result<Option<Task>> {
    let task=c.query_row("SELECT task_id,status,created_at,started_at,finished_at,stop_reason,retry_after_at FROM offer_tasks WHERE task_id=?1",[id],|r|Ok(Task{task_id:r.get(0)?,status:r.get(1)?,created_at:r.get(2)?,started_at:r.get(3)?,finished_at:r.get(4)?,stop_reason:r.get(5)?,retry_after_at:r.get(6)?,total_accounts:0,completed_accounts:0,total_checks:0,completed_checks:0,unknown_checks:0})).optional()?;
    let Some(mut task) = task else {
        return Ok(None);
    };
    let (total,finished,unknown):(usize,usize,usize)=c.query_row("SELECT count(*),coalesce(sum(execution_status='finished'),0),coalesce(sum(eligibility='unknown'),0) FROM offer_checks WHERE task_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    task.total_checks = total;
    task.completed_checks = finished;
    task.unknown_checks = unknown;
    task.total_accounts = total / COUPONS.len();
    task.completed_accounts=c.query_row("SELECT count(*) FROM (SELECT account_id FROM offer_checks WHERE task_id=?1 GROUP BY account_id HAVING sum(execution_status!='finished')=0)",[id],|r|r.get(0))?;
    Ok(Some(task))
}

fn checks_from(c: &Connection, id: &str, account: Option<i64>) -> Result<Vec<Check>> {
    let mut q=c.prepare("SELECT task_id,account_id,email,coupon,execution_status,eligibility,reason,http_status,upstream_state,attempts,checked_at,redemption_status FROM offer_checks WHERE task_id=?1 AND (?2 IS NULL OR account_id=?2) ORDER BY account_id,coupon")?;
    let rows = q
        .query_map(params![id, account], check_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
fn check_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Check> {
    let eligibility: Option<String> = r.get(5)?;
    Ok(Check {
        task_id: r.get(0)?,
        account_id: r.get(1)?,
        email: r.get(2)?,
        coupon: r.get(3)?,
        execution_status: r.get(4)?,
        eligibility: eligibility.map(|s| match s.as_str() {
            "eligible" => Eligibility::Eligible,
            "not_eligible" => Eligibility::NotEligible,
            _ => Eligibility::Unknown,
        }),
        reason: r.get(6)?,
        http_status: r.get(7)?,
        upstream_state: r.get(8)?,
        attempts: r.get(9)?,
        checked_at: r.get(10)?,
        redemption_status: r.get(11)?,
    })
}
