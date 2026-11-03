PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
BEGIN;
CREATE TABLE IF NOT EXISTS offer_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS offer_tasks(
 task_id TEXT PRIMARY KEY, source_key TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('queued','running','completed','interrupted','failed')),
 created_at INTEGER NOT NULL, started_at INTEGER, finished_at INTEGER,
 stop_reason TEXT, retry_after_at INTEGER
);
CREATE TABLE IF NOT EXISTS offer_checks(
 task_id TEXT NOT NULL REFERENCES offer_tasks(task_id), account_id INTEGER NOT NULL,
 email TEXT NOT NULL, coupon TEXT NOT NULL,
 execution_status TEXT NOT NULL CHECK(execution_status IN ('pending','running','finished')),
 eligibility TEXT CHECK(eligibility IN ('eligible','not_eligible','unknown')),
 reason TEXT, http_status INTEGER, upstream_state TEXT,
 attempts INTEGER NOT NULL DEFAULT 0, checked_at INTEGER, redemption_status TEXT,
 PRIMARY KEY(task_id,account_id,email,coupon)
);
CREATE INDEX IF NOT EXISTS offer_tasks_source ON offer_tasks(source_key,created_at);
CREATE INDEX IF NOT EXISTS offer_checks_account ON offer_checks(account_id,email,coupon,checked_at);
PRAGMA user_version=1;
COMMIT;
