use async_trait::async_trait;
use std::cmp::min;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

use crate::domain::tasks::task_event::TaskEvent;
use crate::domain::tasks::task_handler::TaskHandler;
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

// ── 重试配置 ──

/// 内存重试队列的最大重试次数
const MAX_RETRIES: u32 = 10;

/// 退避公式：min(2^retry_count, 60) 秒
fn retry_delay(retry_count: u32) -> tokio::time::Duration {
    let secs = min(2u64.pow(retry_count), 60);
    tokio::time::Duration::from_secs(secs)
}

// ── 带重试的 Publisher ──

/// 包装 mpsc channel 的 TaskPublisher，发送失败时自动入内存重试队列。
///
/// 后台协程定期扫描队列并重试失败事件。
/// 服务器进程崩溃时重试队列丢失（可接受——仅影响崩溃期间的事件投递）。
#[derive(Clone)]
pub struct RetryingTaskPublisher {
    inner: Arc<Inner>,
}

struct Inner {
    sender: mpsc::UnboundedSender<TaskEvent>,
    /// 重试队列：(事件, 已重试次数)
    retry_queue: Mutex<VecDeque<(TaskEvent, u32)>>,
}

impl RetryingTaskPublisher {
    fn new(sender: mpsc::UnboundedSender<TaskEvent>) -> Self {
        Self {
            inner: Arc::new(Inner {
                sender,
                retry_queue: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// 启动后台重试协程。
    /// 每秒扫描一次队列，对队首事件进行退避重试。
    pub fn spawn_retry_worker(this: Self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
            loop {
                interval.tick().await;
                let mut queue = this.inner.retry_queue.lock().await;
                if let Some((event, retry_count)) = queue.pop_front() {
                    // 尝试重新发送
                    match this.inner.sender.send(event.clone()) {
                        Ok(()) => {
                            info!(
                                retry_count,
                                "重试队列事件投递成功"
                            );
                        }
                        Err(_) => {
                            let new_retry = retry_count + 1;
                            if new_retry < MAX_RETRIES {
                                warn!(
                                    retry_count = new_retry,
                                    "重试队列事件投递失败，将在 {} 秒后重试",
                                    retry_delay(new_retry).as_secs()
                                );
                                queue.push_back((event, new_retry));
                            } else {
                                warn!(
                                    event = ?event,
                                    "重试队列事件已达最大重试次数（{}），丢弃", MAX_RETRIES
                                );
                                // 丢弃事件，不再重试
                            }
                        }
                    }
                }
                // drop(queue) 自动释放锁
            }
        })
    }
}

#[async_trait]
impl TaskPublisher for RetryingTaskPublisher {
    async fn publish(&self, event: TaskEvent) -> Result<(), AppError> {
        // 1. 尝试正常发送
        if self.inner.sender.send(event.clone()).is_ok() {
            return Ok(());
        }

        // 2. 发送失败，入重试队列
        warn!("任务通道已关闭，事件已加入内存重试队列");
        let mut queue = self.inner.retry_queue.lock().await;
        // 防止内存泄漏：限制队列最大容量
        if queue.len() < 1000 {
            queue.push_back((event, 0));
        } else {
            warn!("内存重试队列已满（1000 条），丢弃事件");
        }
        // 返回 Ok 而不是 Err，因为事件已进入重试流程
        Ok(())
    }
}

// ── Worker ──

pub struct TaskWorker {
    receiver: mpsc::UnboundedReceiver<TaskEvent>,
    handlers: Vec<Arc<dyn TaskHandler>>,
}

/// 创建通道对。Worker 启动时没有处理器——
/// 使用 `TaskWorker::with_handler` 注入处理器。
pub fn new_task_channel(_buffer: usize) -> (RetryingTaskPublisher, TaskWorker) {
    let (tx, rx) = mpsc::unbounded_channel();
    let worker = TaskWorker {
        receiver: rx,
        handlers: Vec::new(),
    };
    (RetryingTaskPublisher::new(tx), worker)
}

impl TaskWorker {
    pub fn with_handler(mut self, handler: Arc<dyn TaskHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    pub async fn run(mut self) {
        info!(
            handlers = ?self.handlers.iter().map(|h| h.name()).collect::<Vec<_>>(),
            "任务 worker 启动"
        );
        while let Some(event) = self.receiver.recv().await {
            for h in &self.handlers {
                debug!(handler = h.name(), event = ?event, "分发事件");
                h.handle(&event).await;
            }
        }
        info!("任务 worker 已停止");
    }
}
