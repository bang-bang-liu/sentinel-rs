use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Credentials deliberately implement neither Debug nor Serialize.
pub struct Account {
    pub id: i64,
    pub email: String,
    pub password: String,
    pub totp_secret: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountView {
    pub id: i64,
    pub email: String,
    pub status: String,
    pub has_password: bool,
    pub has_access_token: bool,
    pub has_totp: bool,
    pub updated_at: i64,
}

#[derive(Clone, Default, Deserialize)]
pub struct AccountQuery {
    pub q: Option<String>,
    pub account_id: Option<i64>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

impl AccountQuery {
    pub fn validate(&self) -> Result<()> {
        if self.page == Some(0)
            || self.page_size.is_some_and(|n| n == 0 || n > 200)
            || self.q.as_ref().is_some_and(|s| s.len() > 256)
            || self.account_id.is_some_and(|id| id <= 0)
        {
            bail!("invalid_query");
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub struct AccountPage {
    pub accounts: Vec<AccountView>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Clone)]
pub struct Accounts {
    path: PathBuf,
}

impl Accounts {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn connect(&self, write: bool) -> Result<Connection> {
        // Never create a database or migrate the registration schema.
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let conn = Connection::open_with_flags(&self.path, flags)?;
        conn.busy_timeout(std::time::Duration::from_secs(15))?;
        Ok(conn)
    }

    pub fn check(&self) -> Result<()> {
        self.connect(false)?.prepare(
            "SELECT id,email,password,access_token,totp_secret,cookies,status,created_at,updated_at FROM accounts LIMIT 0",
        )?;
        Ok(())
    }

    pub fn get(&self, id: i64) -> Result<Option<Account>> {
        Ok(self
            .connect(false)?
            .query_row(
                "SELECT id,email,password,coalesce(totp_secret,'') FROM accounts WHERE id=?1",
                [id],
                |row| {
                    Ok(Account {
                        id: row.get(0)?,
                        email: row.get(1)?,
                        password: row.get(2)?,
                        totp_secret: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn list(&self, query: &AccountQuery) -> Result<AccountPage> {
        query.validate()?;
        let page = query.page.unwrap_or(1);
        let size = query.page_size.unwrap_or(50);
        let conn = self.connect(false)?;
        let needle = query.q.as_deref().unwrap_or("").trim();
        let filter = "(?1='' OR instr(lower(email),lower(?1))>0) AND (?2 IS NULL OR id=?2)";
        let total = conn.query_row(
            &format!("SELECT count(*) FROM accounts WHERE {filter}"),
            params![needle, query.account_id],
            |r| r.get(0),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id,email,status,length(trim(coalesce(password,'')))>0,\
             length(trim(coalesce(access_token,'')))>0,length(trim(coalesce(totp_secret,'')))>0,\
             cast(updated_at AS INTEGER) FROM accounts WHERE {filter} ORDER BY id DESC LIMIT ?3 OFFSET ?4"
        ))?;
        let accounts = stmt
            .query_map(
                params![
                    needle,
                    query.account_id,
                    size,
                    i64::from(page - 1) * i64::from(size)
                ],
                |row| {
                    Ok(AccountView {
                        id: row.get(0)?,
                        email: row.get(1)?,
                        status: row.get(2)?,
                        has_password: row.get(3)?,
                        has_access_token: row.get(4)?,
                        has_totp: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(AccountPage {
            accounts,
            total,
            page,
            page_size: size,
        })
    }

    pub fn save_login(&self, id: i64, login: &crate::login::LoginOutput) -> Result<()> {
        if login.access_token.trim().is_empty() || login.cookies.trim().is_empty() {
            bail!("empty_credentials");
        }
        let conn = self.connect(true)?;
        let count = conn
            .execute(
                "UPDATE accounts SET access_token=?1,cookies=?2,\
             totp_secret=CASE WHEN ?3 IS NULL THEN totp_secret ELSE ?3 END,\
             status='registered',updated_at=?4 WHERE id=?5",
                params![
                    login.access_token,
                    login.cookies,
                    login.new_totp_secret,
                    crate::now(),
                    id
                ],
            )
            .context("account_update_failed")?;
        if count != 1 {
            bail!("account_not_found");
        }
        Ok(())
    }
}
