use crate::domain::model::{Account, Credential};
use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Source {
    path: PathBuf,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let this = Self {
            path: path.canonicalize().context("source_db_missing")?,
        };
        let c = this.connection()?;
        let mut q = c.prepare("PRAGMA table_info(accounts)")?;
        let fields = q
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (name, kind) in [
            ("id", "INTEGER"),
            ("email", "TEXT"),
            ("status", "TEXT"),
            ("access_token", "TEXT"),
        ] {
            if !fields
                .iter()
                .any(|(n, t)| n == name && t.eq_ignore_ascii_case(kind))
            {
                bail!("source_schema_incompatible: {name}");
            }
        }
        Ok(this)
    }
    pub fn key(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
    pub fn connection(&self) -> Result<Connection> {
        let c = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("source_db_unreadable")?;
        c.busy_timeout(Duration::from_secs(3))?;
        c.execute_batch("PRAGMA query_only=ON;")?;
        Ok(c)
    }
    pub fn accounts(&self) -> Result<Vec<Account>> {
        let c = self.connection()?;
        let mut s=c.prepare("SELECT id,email,status,coalesce(length(trim(access_token))>0,0) FROM accounts ORDER BY id DESC")?;
        let rows = s
            .query_map([], |r| {
                Ok(Account {
                    id: r.get(0)?,
                    email: r.get(1)?,
                    status: r.get(2)?,
                    has_token: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
    pub fn credential(&self, id: i64, email: &str) -> Result<Option<Credential>> {
        let c = self.connection()?;
        Ok(c.query_row(
            "SELECT coalesce(access_token,'') FROM accounts WHERE id=?1 AND email=?2",
            params![id, email],
            |r| {
                Ok(Credential {
                    access_token: r.get::<_, String>(0)?.trim().into(),
                })
            },
        )
        .optional()?)
    }
}
