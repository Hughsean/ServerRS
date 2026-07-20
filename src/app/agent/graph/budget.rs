use super::{BudgetResource, GraphRunError, RunId};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphPolicy {
    max_steps: NonZeroU32,
}

impl GraphPolicy {
    pub fn new(max_steps: NonZeroU32) -> Self {
        Self { max_steps }
    }

    pub fn max_steps(&self) -> NonZeroU32 {
        self.max_steps
    }
}

/// 单次图运行的硬资源限制。
#[derive(Debug, Clone, Copy)]
pub struct RunBudget {
    max_steps: NonZeroU32,
    max_llm_calls: Option<u32>,
    max_tool_calls: Option<u32>,
    max_tokens: Option<u64>,
    max_duration: Duration,
}

impl RunBudget {
    pub fn new(max_steps: NonZeroU32, max_duration: Duration) -> Self {
        Self {
            max_steps,
            max_llm_calls: None,
            max_tool_calls: None,
            max_tokens: None,
            max_duration,
        }
    }

    pub fn with_llm_calls(mut self, max_llm_calls: u32) -> Self {
        self.max_llm_calls = Some(max_llm_calls);
        self
    }

    pub fn with_tool_calls(mut self, max_tool_calls: u32) -> Self {
        self.max_tool_calls = Some(max_tool_calls);
        self
    }

    pub fn with_tokens(mut self, max_tokens: u64) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn max_steps(&self) -> NonZeroU32 {
        self.max_steps
    }

    pub fn max_llm_calls(&self) -> Option<u32> {
        self.max_llm_calls
    }

    pub fn max_tool_calls(&self) -> Option<u32> {
        self.max_tool_calls
    }

    pub fn max_tokens(&self) -> Option<u64> {
        self.max_tokens
    }

    pub fn max_duration(&self) -> Duration {
        self.max_duration
    }

    #[cfg(test)]
    pub fn for_test(max_steps: u32) -> Self {
        Self::new(
            NonZeroU32::new(max_steps).expect("test budget max_steps must be nonzero"),
            Duration::from_secs(30),
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageDelta {
    pub tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UsageSnapshot {
    pub steps: u32,
    pub llm_calls: u32,
    pub tool_calls: u32,
    pub tokens: u64,
}

#[derive(Debug, Clone)]
pub struct RunBudgetHandle {
    limits: RunBudget,
    usage: Arc<Mutex<UsageSnapshot>>,
}

impl RunBudgetHandle {
    pub fn new(limits: RunBudget) -> Self {
        Self {
            limits,
            usage: Arc::new(Mutex::new(UsageSnapshot::default())),
        }
    }

    pub fn limits(&self) -> RunBudget {
        self.limits
    }

    pub fn snapshot(&self) -> UsageSnapshot {
        *self
            .usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 在外部调用前原子预留并记录一次 LLM 调用。
    pub fn reserve_llm_call(&self) -> Result<(), GraphRunError> {
        let mut usage = self.lock_usage();
        let next = check_optional_limit(
            BudgetResource::LlmCalls,
            u64::from(usage.llm_calls),
            1,
            self.limits.max_llm_calls.map(u64::from),
            u64::from(u32::MAX),
        )?;
        usage.llm_calls = next as u32;
        Ok(())
    }

    /// 在执行前一次性预留并记录节点即将发起的工具调用总数。
    pub fn reserve_tool_calls(&self, count: u32) -> Result<(), GraphRunError> {
        let mut usage = self.lock_usage();
        let next = check_optional_limit(
            BudgetResource::ToolCalls,
            u64::from(usage.tool_calls),
            u64::from(count),
            self.limits.max_tool_calls.map(u64::from),
            u64::from(u32::MAX),
        )?;
        usage.tool_calls = next as u32;
        Ok(())
    }

    pub fn record_tokens(&self, count: u64) -> Result<(), GraphRunError> {
        let mut usage = self.lock_usage();
        let next = check_optional_limit(
            BudgetResource::Tokens,
            usage.tokens,
            count,
            self.limits.max_tokens,
            u64::MAX,
        )?;
        usage.tokens = next;
        Ok(())
    }

    /// 记录节点完成后才能获知的实际 Token 用量。
    /// LLM 与工具调用次数必须在外部调用前通过 reserve 方法预留，避免双计数。
    pub fn record_usage(&self, delta: UsageDelta) -> Result<(), GraphRunError> {
        self.record_tokens(delta.tokens)
    }

    pub(crate) fn reserve_step(&self, graph_max_steps: NonZeroU32) -> Result<(), GraphRunError> {
        let mut usage = self.lock_usage();
        let limit = self.limits.max_steps.get().min(graph_max_steps.get());
        let attempted = usage.steps.saturating_add(1);
        if attempted > limit {
            return Err(GraphRunError::BudgetExceeded {
                resource: BudgetResource::Steps,
                limit: u64::from(limit),
                attempted: u64::from(attempted),
            });
        }
        usage.steps = attempted;
        Ok(())
    }

    fn lock_usage(&self) -> std::sync::MutexGuard<'_, UsageSnapshot> {
        self.usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn check_optional_limit(
    resource: BudgetResource,
    current: u64,
    delta: u64,
    configured_limit: Option<u64>,
    representation_limit: u64,
) -> Result<u64, GraphRunError> {
    let attempted = current
        .checked_add(delta)
        .ok_or(GraphRunError::BudgetExceeded {
            resource,
            limit: configured_limit.unwrap_or(representation_limit),
            attempted: u64::MAX,
        })?;
    let limit = configured_limit.unwrap_or(representation_limit);
    if attempted > limit {
        return Err(GraphRunError::BudgetExceeded {
            resource,
            limit,
            attempted,
        });
    }
    Ok(attempted)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunTrace {
    pub trace_id: Option<String>,
    pub attributes: BTreeMap<String, String>,
}

/// 仅保存一次运行共享的横切信息，不承载业务依赖。
#[derive(Debug, Clone)]
pub struct RunContext {
    run_id: RunId,
    budget: RunBudgetHandle,
    cancellation: CancellationToken,
    deadline: Instant,
    trace: RunTrace,
}

impl RunContext {
    pub fn new(budget: RunBudget, cancellation: CancellationToken, trace: RunTrace) -> Self {
        Self {
            run_id: RunId::new(),
            budget: RunBudgetHandle::new(budget),
            cancellation,
            deadline: Instant::now() + budget.max_duration(),
            trace,
        }
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn budget(&self) -> &RunBudgetHandle {
        &self.budget
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn trace(&self) -> &RunTrace {
        &self.trace
    }

    pub(crate) fn check_ready(&self, graph_max_steps: NonZeroU32) -> Result<(), GraphRunError> {
        if self.cancellation.is_cancelled() {
            return Err(GraphRunError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(GraphRunError::DeadlineExceeded);
        }
        self.budget.reserve_step(graph_max_steps)
    }
}
