use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::auth::SessionStore;

/// 全局共享状态（Clone 传给各路由层与中间件）
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub sessions: SessionStore,
    pub session_ttl_hours: u64,
    /// v3 批量分析任务注册表（进程内；单用户局域网足够，重启即清）
    pub batch_jobs: BatchJobs,
    /// AI 任务注册表 + 事件流（ADR-0013）：同步 LLM 出口幂等化，SSE 回显
    pub ai_jobs: AiJobs,
    /// v5 领域事件总线（ADR-0017 D3）：解耦积分、复习队列、状态流水等副作用
    pub event_bus: crate::events::EventBus,
}

impl AppState {
    pub fn session_ttl(&self) -> Duration {
        Duration::from_secs(self.session_ttl_hours * 3600)
    }
}

/// AI 任务事件（ADR-0013 D3）：状态变化广播给 SSE 订阅者（连接按 uid 过滤转发）
#[derive(Clone, Debug, serde::Serialize)]
pub struct AiEvent {
    pub uid: i64,
    pub job_id: u64,
    pub kind: String,
    pub target_id: i64,
    /// running | done | failed
    pub status: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct AiJob {
    pub id: u64,
    pub uid: i64,
    /// ref / analyze / jd_interpret / jd_match / resume_parse / retrospective /
    /// overall / app_insights / interview_prep / position_predict（词表见 docs/context.md）
    pub kind: String,
    pub target_id: i64,
    /// running | done | failed
    pub status: String,
    pub started_at: DateTime<Utc>,
}

/// 幂等入口结果：已有同键 running 任务则返回既有（调用方直接回显，不重复起 LLM）
pub enum AiStart {
    Started(AiJob),
    AlreadyRunning(AiJob),
}

/// AI 任务注册表（ADR-0013 D1）：进程内；键 (uid, kind, target_id) 幂等去重；
/// 终态任务保留最近 MAX_FINISHED 条供 GET /ai-jobs/:id 轮询兜底。重启即清（与 batch_jobs 同先例），
/// 已持久化结果不受影响。
#[derive(Clone)]
pub struct AiJobs {
    next_id: Arc<AtomicU64>,
    running: Arc<Mutex<HashMap<(i64, String, i64), AiJob>>>,
    finished: Arc<Mutex<VecDeque<AiJob>>>,
    events: broadcast::Sender<AiEvent>,
}

const MAX_FINISHED: usize = 512;

impl AiJobs {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            next_id: Arc::new(AtomicU64::new(0)),
            running: Arc::new(Mutex::new(HashMap::new())),
            finished: Arc::new(Mutex::new(VecDeque::new())),
            events,
        }
    }

    /// 幂等起点：同 (uid,kind,target) 已有 running → AlreadyRunning(既有 job)；否则登记新任务并广播 running。
    pub fn start(&self, uid: i64, kind: &str, target_id: i64) -> AiStart {
        let key = (uid, kind.to_string(), target_id);
        let mut r = self.running.lock().unwrap();
        if let Some(j) = r.get(&key) {
            return AiStart::AlreadyRunning(j.clone());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let job = AiJob {
            id,
            uid,
            kind: kind.to_string(),
            target_id,
            status: "running".into(),
            started_at: Utc::now(),
        };
        r.insert(key, job.clone());
        drop(r);
        let _ = self.events.send(AiEvent {
            uid,
            job_id: id,
            kind: kind.to_string(),
            target_id,
            status: "running".into(),
        });
        AiStart::Started(job)
    }

    /// 任务收尾：移出 running、记入 finished（截断）、广播终态事件。
    /// 只清自己这一单（同键若被新一轮覆盖不误删——防御性）。
    pub fn finish(&self, job: &AiJob, ok: bool) {
        let key = (job.uid, job.kind.clone(), job.target_id);
        {
            let mut r = self.running.lock().unwrap();
            if r.get(&key).map(|j| j.id) == Some(job.id) {
                r.remove(&key);
            }
        }
        let status = if ok { "done" } else { "failed" }.to_string();
        {
            let mut f = self.finished.lock().unwrap();
            f.push_back(AiJob {
                status: status.clone(),
                ..job.clone()
            });
            while f.len() > MAX_FINISHED {
                f.pop_front();
            }
        }
        tracing::info!(job_id = job.id, kind = %job.kind, target_id = job.target_id, ok, "AI 任务完成");
        let _ = self.events.send(AiEvent {
            uid: job.uid,
            job_id: job.id,
            kind: job.kind.clone(),
            target_id: job.target_id,
            status,
        });
    }

    /// 按 job_id 查（running 或最近终态）；轮询兜底通道
    pub fn get(&self, job_id: u64) -> Option<AiJob> {
        for j in self.running.lock().unwrap().values() {
            if j.id == job_id {
                return Some(j.clone());
            }
        }
        self.finished.lock().unwrap().iter().find(|j| j.id == job_id).cloned()
    }

    /// 某 (uid,kind,target) 当前 running 的任务（域 GET 暴露 ai_jobs 用）
    pub fn running_for(&self, uid: i64, kind: &str, target_id: i64) -> Option<AiJob> {
        self.running
            .lock()
            .unwrap()
            .get(&(uid, kind.to_string(), target_id))
            .cloned()
    }

    /// 后台执行 AI 任务（panic 守卫，评审 P0）：无论任务正常失败还是 panic，
    /// 都保证 finish() 收尾——否则 running 条目泄漏会让同键幂等去重永久阻塞该出口
    /// （直到重启进程）。所有 ai_jobs 后台任务必须经此入口，不得裸 tokio::spawn。
    ///
    /// 说明：release 构建 panic=abort（进程直接退出，任务级守卫无从谈起）；
    /// 本守卫服务于 dev/测试形态下的任务级隔离与最坏情况收敛。
    pub fn spawn_guarded<F>(&self, job: AiJob, fut: F)
    where
        F: std::future::Future<Output = Result<(), crate::error::AppError>> + Send + 'static,
    {
        let jobs = self.clone();
        tokio::spawn(async move {
            use futures_util::FutureExt;
            match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
                Ok(Ok(())) => jobs.finish(&job, true),
                Ok(Err(e)) => {
                    tracing::error!(
                        job_id = job.id, kind = %job.kind, target_id = job.target_id,
                        error = %e, "AI 任务失败"
                    );
                    jobs.finish(&job, false);
                }
                Err(payload) => {
                    let msg = panic_message(&payload);
                    tracing::error!(
                        job_id = job.id, kind = %job.kind, target_id = job.target_id,
                        panic = %msg, "AI 任务 panic（守卫兜底收尾）"
                    );
                    jobs.finish(&job, false);
                }
            }
        });
    }

    pub fn publish_event(&self, ev: AiEvent) {
        let _ = self.events.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AiEvent> {
        self.events.subscribe()
    }
}

/// 从 panic payload 提取可读信息（&str/String 之外的原值给占位描述）
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0013 D1 单测：同键幂等去重、finish 解锁、get 可查终态、事件广播可见
    #[test]
    fn ai_jobs_start_dedupes_running_and_finish_unlocks() {
        let jobs = AiJobs::new();
        let mut rx = jobs.subscribe();
        let j1 = match jobs.start(1, "ref", 42) {
            AiStart::Started(j) => j,
            AiStart::AlreadyRunning(_) => panic!("首次 start 不应去重"),
        };
        // 同键再 start：拿到同一 job（幂等）
        match jobs.start(1, "ref", 42) {
            AiStart::Started(_) => panic!("running 中不应起新任务"),
            AiStart::AlreadyRunning(j) => assert_eq!(j.id, j1.id),
        }
        // 不同 kind / target / uid 各自独立
        assert!(matches!(jobs.start(1, "analyze", 42), AiStart::Started(_)));
        assert!(matches!(jobs.start(1, "ref", 43), AiStart::Started(_)));
        assert!(matches!(jobs.start(2, "ref", 42), AiStart::Started(_)));
        // running 事件广播可见
        let ev = rx.try_recv().expect("应有 running 事件");
        assert_eq!((ev.uid, ev.kind.as_str(), ev.status.as_str()), (1, "ref", "running"));
        // 收尾后可重跑（新 id），get 能查到终态，done 事件广播
        jobs.finish(&j1, true);
        assert_eq!(jobs.get(j1.id).unwrap().status, "done");
        assert!(matches!(jobs.start(1, "ref", 42), AiStart::Started(j2) if j2.id != j1.id));
        // 失败收尾同样解锁并可见 failed
        let j2 = jobs.running_for(1, "ref", 42).unwrap();
        jobs.finish(&j2, false);
        assert_eq!(jobs.get(j2.id).unwrap().status, "failed");
    }

    /// 评审 P0：spawn_guarded 的 panic 守卫——future panic 时 running 条目也必须被释放，
    /// 终态为 failed；同键可重新发起（幂等去重不被永久阻塞）。
    #[tokio::test]
    async fn spawn_guarded_releases_running_entry_even_on_panic() {
        let jobs = AiJobs::new();
        let j = match jobs.start(7, "ref", 9) {
            AiStart::Started(j) => j,
            AiStart::AlreadyRunning(_) => panic!("首次 start 不应去重"),
        };
        jobs.spawn_guarded(j.clone(), async {
            panic!("boom");
            #[allow(unreachable_code)]
            Ok::<_, crate::error::AppError>(())
        });
        for _ in 0..200 {
            if jobs.get(j.id).map(|x| x.status != "running").unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(jobs.get(j.id).unwrap().status, "failed", "panic 应以 failed 收尾");
        assert!(jobs.running_for(7, "ref", 9).is_none(), "running 条目必须被释放");
        // 同键可重新发起
        assert!(matches!(jobs.start(7, "ref", 9), AiStart::Started(_)));
    }

    /// 正常失败路径同样经守卫收尾并记录 failed。
    #[tokio::test]
    async fn spawn_guarded_records_failure_result() {
        let jobs = AiJobs::new();
        let j = match jobs.start(1, "analyze", 2) {
            AiStart::Started(j) => j,
            AiStart::AlreadyRunning(_) => panic!("首次 start 不应去重"),
        };
        jobs.spawn_guarded(j.clone(), async {
            Err(crate::error::AppError::BadRequest("业务失败".into()))
        });
        for _ in 0..200 {
            if jobs.get(j.id).map(|x| x.status != "running").unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(jobs.get(j.id).unwrap().status, "failed");
    }

    #[test]
    fn panic_message_extracts_str_and_string() {
        assert_eq!(panic_message(&"文本 panic"), "文本 panic");
        assert_eq!(panic_message(&String::from("owned panic")), "owned panic");
        assert_eq!(panic_message(&42_i32), "unknown panic");
    }
}

/// 批量分析任务（M1）：逐题跑 `run_analysis`，进度经 GET 轮询，DELETE 取消。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub struct BatchJob {
    pub id: u64,
    /// 归属用户：GET/DELETE 必须校验（多用户行级隔离，评审 P1）。不回传前端。
    #[serde(skip)]
    pub uid: i64,
    pub status: String, // running | done | cancelled | error
    pub total: usize,
    pub done: usize,
    pub ok: usize,
    pub failed: usize,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
}

/// 进程内任务注册表：id 自增 + 取消标记。单用户，不做持久化。
#[derive(Clone, Default)]
pub struct BatchJobs {
    next_id: Arc<AtomicU64>,
    jobs: Arc<Mutex<HashMap<u64, BatchJob>>>,
    cancelled: Arc<Mutex<HashMap<u64, bool>>>,
}

impl BatchJobs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&self, uid: i64, total: usize) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let job = BatchJob {
            id,
            uid,
            status: "running".into(),
            total,
            done: 0,
            ok: 0,
            failed: 0,
            error: None,
            started_at: Utc::now(),
        };
        self.jobs.lock().unwrap().insert(id, job);
        self.cancelled.lock().unwrap().insert(id, false);
        id
    }

    pub fn get(&self, id: u64) -> Option<BatchJob> {
        self.jobs.lock().unwrap().get(&id).cloned()
    }

    pub fn is_cancelled(&self, id: u64) -> bool {
        *self.cancelled.lock().unwrap().get(&id).unwrap_or(&false)
    }

    pub fn cancel(&self, id: u64) -> bool {
        if let Some(c) = self.cancelled.lock().unwrap().get_mut(&id) {
            *c = true;
            true
        } else {
            false
        }
    }

    pub fn update<F: FnOnce(&mut BatchJob)>(&self, id: u64, f: F) {
        if let Some(mut job) = self.jobs.lock().unwrap().get_mut(&id) {
            f(&mut job);
        }
    }
}
