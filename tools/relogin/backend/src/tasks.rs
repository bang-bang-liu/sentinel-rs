use crate::{
    accounts::{Account, Accounts},
    login::Runner,
    now,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Session,
    Password,
    Email,
    Totp,
    Token,
    Enroll,
    Save,
    Done,
}

pub type Progress = Arc<dyn Fn(Stage) + Send + Sync>;

#[derive(Clone, Serialize, Deserialize)]
pub struct StageEvent {
    pub stage: Stage,
    pub at: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Task {
    pub task_id: String,
    pub account_id: i64,
    pub email: String,
    pub status: String,
    pub stage: Stage,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub message: Option<String>,
    pub enrolled_2fa: bool,
    pub events: Vec<StageEvent>,
}

#[derive(Default)]
struct State {
    tasks: Vec<Task>,
}

#[derive(Clone, Default)]
pub struct Tasks {
    state: Arc<Mutex<State>>,
}

impl Tasks {
    pub fn latest(&self) -> Option<Task> {
        self.state.lock().unwrap().tasks.last().cloned()
    }
    pub fn get(&self, id: &str) -> Option<Task> {
        self.state
            .lock()
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.task_id == id)
            .cloned()
    }

    pub fn start(
        &self,
        account: Account,
        accounts: Accounts,
        runner: Arc<dyn Runner>,
    ) -> Result<String, Task> {
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut state = self.state.lock().unwrap();
            if let Some(active) = state.tasks.iter().find(|t| t.status == "running") {
                return Err(active.clone());
            }
            if state.tasks.len() >= 100 {
                state.tasks.remove(0);
            }
            state.tasks.push(Task {
                task_id: id.clone(),
                account_id: account.id,
                email: account.email.clone(),
                status: "running".into(),
                stage: Stage::Session,
                started_at: now(),
                finished_at: None,
                message: None,
                enrolled_2fa: false,
                events: vec![StageEvent {
                    stage: Stage::Session,
                    at: now(),
                }],
            });
        }
        let progress_state = self.state.clone();
        let progress_id = id.clone();
        let progress: Progress = Arc::new(move |stage| {
            if let Some(task) = progress_state
                .lock()
                .unwrap()
                .tasks
                .iter_mut()
                .find(|t| t.task_id == progress_id)
            {
                if task.stage != stage {
                    task.stage = stage;
                    task.events.push(StageEvent { stage, at: now() });
                    tracing::info!(task_id = %task.task_id, account_id = task.account_id, ?stage, "relogin stage");
                }
            }
        });
        let worker = tokio::spawn(async move {
            let output = runner.run(&account, progress.clone()).await?;
            progress(Stage::Save);
            let enrolled = output.new_totp_secret.is_some();
            tokio::task::spawn_blocking(move || accounts.save_login(account.id, &output))
                .await
                .map_err(|_| anyhow::anyhow!("账号保存任务失败"))?
                .map_err(|_| anyhow::anyhow!("账号写入失败，请检查数据库权限或锁定状态"))?;
            Ok::<bool, anyhow::Error>(enrolled)
        });
        let state = self.state.clone();
        let finish_id = id.clone();
        tokio::spawn(async move {
            let result = worker.await;
            let mut state = state.lock().unwrap();
            let task = state
                .tasks
                .iter_mut()
                .find(|t| t.task_id == finish_id)
                .unwrap();
            task.finished_at = Some(now());
            match result {
                Ok(Ok(enrolled)) => {
                    task.status = "succeeded".into();
                    task.stage = Stage::Done;
                    task.enrolled_2fa = enrolled;
                    task.message = Some(
                        if enrolled {
                            "已更新 Access Token、Cookie，并补绑 2FA"
                        } else {
                            "已更新 Access Token 和 Cookie"
                        }
                        .into(),
                    );
                    task.events.push(StageEvent {
                        stage: Stage::Done,
                        at: now(),
                    });
                }
                Ok(Err(err)) => {
                    task.status = "failed".into();
                    task.message = Some(err.to_string());
                }
                Err(_) => {
                    task.status = "failed".into();
                    task.message = Some("重登任务异常结束".into());
                }
            }
            tracing::info!(task_id = %task.task_id, account_id = task.account_id, status = %task.status, "relogin finished");
        });
        Ok(id)
    }
}
