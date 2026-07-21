use crate::app::memory::memory_service::MemoryService;
use crate::domain::llm::ChatMessage;
use crate::shared::error::AppError;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, warn};

const RETRY_DELAY: Duration = Duration::from_millis(500);
const FAILURE_COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryExtractionRequest {
    pub user_id: u64,
    pub conversation_id: u64,
    pub source_message_id: u64,
    pub user_message: String,
    pub assistant_reply: String,
    pub context_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryExtractionDispatch {
    Scheduled,
    SkippedRecentFailure,
}

/// 接受图中的记忆提取请求，并保持 HTTP Chat 原有的即发即忘语义。
pub trait MemoryExtractionSchedulerT: Send + Sync {
    fn schedule(&self, request: MemoryExtractionRequest) -> MemoryExtractionDispatch;
}

#[async_trait]
trait MemoryExtractionWorkerT: Send + Sync {
    async fn extract_and_save(&self, request: &MemoryExtractionRequest) -> Result<usize, AppError>;
}

struct MemoryServiceExtractionWorker {
    service: Arc<MemoryService>,
}

#[async_trait]
impl MemoryExtractionWorkerT for MemoryServiceExtractionWorker {
    async fn extract_and_save(&self, request: &MemoryExtractionRequest) -> Result<usize, AppError> {
        let messages = vec![
            ChatMessage {
                role: "user".into(),
                content: request.user_message.clone(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "assistant".into(),
                content: request.assistant_reply.clone(),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];
        self.service
            .extract_and_save_at_version(
                request.user_id,
                &messages,
                request.conversation_id,
                request.source_message_id,
                Some(request.context_version),
            )
            .await
            .map(|memories| memories.len())
    }
}

pub struct AsyncMemoryExtractionScheduler {
    worker: Arc<dyn MemoryExtractionWorkerT>,
    last_failure: Arc<Mutex<Option<Instant>>>,
    retry_delay: Duration,
    failure_cooldown: Duration,
}

impl AsyncMemoryExtractionScheduler {
    pub fn new(service: Arc<MemoryService>) -> Self {
        Self {
            worker: Arc::new(MemoryServiceExtractionWorker { service }),
            last_failure: Arc::new(Mutex::new(None)),
            retry_delay: RETRY_DELAY,
            failure_cooldown: FAILURE_COOLDOWN,
        }
    }

    #[cfg(test)]
    fn with_worker(
        worker: Arc<dyn MemoryExtractionWorkerT>,
        retry_delay: Duration,
        failure_cooldown: Duration,
    ) -> Self {
        Self {
            worker,
            last_failure: Arc::new(Mutex::new(None)),
            retry_delay,
            failure_cooldown,
        }
    }
}

impl MemoryExtractionSchedulerT for AsyncMemoryExtractionScheduler {
    fn schedule(&self, request: MemoryExtractionRequest) -> MemoryExtractionDispatch {
        if let Ok(guard) = self.last_failure.lock()
            && let Some(last_failure) = *guard
            && last_failure.elapsed() < self.failure_cooldown
        {
            debug!(
                user_id = request.user_id,
                conversation_id = request.conversation_id,
                seconds_since_failure = last_failure.elapsed().as_secs(),
                "skipping memory extraction (recent failure)"
            );
            return MemoryExtractionDispatch::SkippedRecentFailure;
        }

        debug!(
            user_id = request.user_id,
            conversation_id = request.conversation_id,
            "启动异步记忆提取"
        );
        let worker = Arc::clone(&self.worker);
        let last_failure = Arc::clone(&self.last_failure);
        let retry_delay = self.retry_delay;
        tokio::spawn(async move {
            let result = match worker.extract_and_save(&request).await {
                Ok(count) => Ok(count),
                Err(error) => {
                    warn!(
                        user_id = request.user_id,
                        conversation_id = request.conversation_id,
                        error = %error,
                        "memory extraction failed (will retry once)"
                    );
                    tokio::time::sleep(retry_delay).await;
                    worker.extract_and_save(&request).await
                }
            };

            match result {
                Ok(count) => debug!(
                    user_id = request.user_id,
                    conversation_id = request.conversation_id,
                    count,
                    "异步记忆提取完成"
                ),
                Err(error) => {
                    warn!(
                        user_id = request.user_id,
                        conversation_id = request.conversation_id,
                        error = %error,
                        "memory extraction failed after retry"
                    );
                    if let Ok(mut guard) = last_failure.lock() {
                        *guard = Some(Instant::now());
                    }
                }
            }
        });
        MemoryExtractionDispatch::Scheduled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct ScriptedWorker {
        calls: AtomicUsize,
        results: Mutex<VecDeque<Result<usize, AppError>>>,
        completed: Notify,
    }

    impl ScriptedWorker {
        fn new(results: Vec<Result<usize, AppError>>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                results: Mutex::new(results.into()),
                completed: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl MemoryExtractionWorkerT for ScriptedWorker {
        async fn extract_and_save(
            &self,
            _request: &MemoryExtractionRequest,
        ) -> Result<usize, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let result = self.results.lock().unwrap().pop_front().unwrap();
            if self.results.lock().unwrap().is_empty() {
                self.completed.notify_one();
            }
            result
        }
    }

    fn request() -> MemoryExtractionRequest {
        MemoryExtractionRequest {
            user_id: 7,
            conversation_id: 9,
            source_message_id: 101,
            user_message: "hello".into(),
            assistant_reply: "world".into(),
            context_version: 23,
        }
    }

    #[tokio::test]
    async fn scheduler_returns_immediately_and_retries_once_in_background() {
        let worker = Arc::new(ScriptedWorker::new(vec![
            Err(AppError::Internal("temporary".into())),
            Ok(2),
        ]));
        let scheduler = AsyncMemoryExtractionScheduler::with_worker(
            worker.clone(),
            Duration::ZERO,
            Duration::from_secs(30),
        );

        assert_eq!(
            scheduler.schedule(request()),
            MemoryExtractionDispatch::Scheduled
        );
        worker.completed.notified().await;

        assert_eq!(worker.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn scheduler_throttles_new_requests_after_retry_failure() {
        let worker = Arc::new(ScriptedWorker::new(vec![
            Err(AppError::Internal("first".into())),
            Err(AppError::Internal("second".into())),
        ]));
        let scheduler = AsyncMemoryExtractionScheduler::with_worker(
            worker.clone(),
            Duration::ZERO,
            Duration::from_secs(30),
        );

        assert_eq!(
            scheduler.schedule(request()),
            MemoryExtractionDispatch::Scheduled
        );
        worker.completed.notified().await;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if scheduler.last_failure.lock().unwrap().is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        assert_eq!(
            scheduler.schedule(request()),
            MemoryExtractionDispatch::SkippedRecentFailure
        );
        assert_eq!(worker.calls.load(Ordering::SeqCst), 2);
    }
}
