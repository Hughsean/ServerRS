use crate::{AgentAction, AgentMessage, AgentObservation, AgentOutcome};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// Prompt 片段的来源，用于保留信任边界和审计信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptSource {
    System,
    Conversation,
    Summary,
    Memory,
    Rag,
    FreshContext,
    Profile,
    Location,
    Business(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromptTrust {
    Trusted,
    Untrusted,
}

/// 进入 Prompt 渲染阶段前的结构化上下文片段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSection {
    pub key: String,
    pub content: String,
    pub source: PromptSource,
    pub trust: PromptTrust,
    pub priority: i32,
}

impl PromptSection {
    pub fn new(
        key: impl Into<String>,
        content: impl Into<String>,
        source: PromptSource,
        trust: PromptTrust,
        priority: i32,
    ) -> Self {
        Self {
            key: key.into(),
            content: content.into(),
            source,
            trust,
            priority,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateSchemaVersion(NonZeroU32);

impl StateSchemaVersion {
    pub const fn initial() -> Self {
        Self(NonZeroU32::MIN)
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u32> for StateSchemaVersion {
    type Error = &'static str;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or("StateSchemaVersion 必须大于 0")
    }
}

/// 业务状态扩展协议。节点只能通过显式 Update 修改业务字段。
pub trait AgentBusinessState: Clone + Send + Sync + 'static {
    type Update: Send + Sync + 'static;
    type Effect: Send + Sync + 'static;
    type SuspendData: Clone + Send + Sync + 'static;
    type ResumeInput: Send + Sync + 'static;

    fn state_schema_version() -> StateSchemaVersion {
        StateSchemaVersion::initial()
    }

    fn resume_updates(input: Self::ResumeInput) -> Vec<AgentUpdate<Self::Update>>;

    fn apply_update(&mut self, update: Self::Update) -> Result<(), AgentStateError>;
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AgentStateError {
    #[error("Agent 已进入终态，只允许业务收尾更新")]
    TerminalState,
    #[error("业务状态更新失败: {0}")]
    Business(String),
}

/// 标准 Agent 状态与类型化业务扩展。
#[derive(Debug, Clone)]
pub struct AgentState<B: AgentBusinessState> {
    messages: Vec<AgentMessage>,
    prompt_sections: Vec<PromptSection>,
    pending_actions: Vec<AgentAction>,
    observations: Vec<AgentObservation>,
    outcome: Option<AgentOutcome>,
    business: B,
}

impl<B: AgentBusinessState> AgentState<B> {
    pub fn new(business: B) -> Self {
        Self {
            messages: Vec::new(),
            prompt_sections: Vec::new(),
            pending_actions: Vec::new(),
            observations: Vec::new(),
            outcome: None,
            business,
        }
    }

    pub fn messages(&self) -> &[AgentMessage] {
        &self.messages
    }

    pub fn prompt_sections(&self) -> &[PromptSection] {
        &self.prompt_sections
    }

    pub fn pending_actions(&self) -> &[AgentAction] {
        &self.pending_actions
    }

    pub fn observations(&self) -> &[AgentObservation] {
        &self.observations
    }

    pub fn outcome(&self) -> Option<&AgentOutcome> {
        self.outcome.as_ref()
    }

    pub fn business(&self) -> &B {
        &self.business
    }

    /// 在候选副本上应用整批更新，全部成功后才提交。
    pub fn apply_updates(
        &mut self,
        updates: Vec<AgentUpdate<B::Update>>,
    ) -> Result<(), AgentStateError> {
        let mut candidate = self.clone();
        for update in updates {
            candidate.apply_one(update)?;
        }
        *self = candidate;
        Ok(())
    }

    fn apply_one(&mut self, update: AgentUpdate<B::Update>) -> Result<(), AgentStateError> {
        if self.outcome.is_some() && !matches!(&update, AgentUpdate::Business(_)) {
            return Err(AgentStateError::TerminalState);
        }

        match update {
            AgentUpdate::AppendMessages(messages) => self.messages.extend(messages),
            AgentUpdate::AppendPromptSections(sections) => self.prompt_sections.extend(sections),
            AgentUpdate::ReplacePendingActions(actions) => self.pending_actions = actions,
            AgentUpdate::AppendObservations(observations) => self.observations.extend(observations),
            AgentUpdate::SetOutcome(outcome) => self.outcome = Some(outcome),
            AgentUpdate::Business(update) => self.business.apply_update(update)?,
        }

        Ok(())
    }
}

/// 节点允许产生的显式状态变化。
#[derive(Debug)]
pub enum AgentUpdate<U> {
    AppendMessages(Vec<AgentMessage>),
    AppendPromptSections(Vec<PromptSection>),
    ReplacePendingActions(Vec<AgentAction>),
    AppendObservations(Vec<AgentObservation>),
    SetOutcome(AgentOutcome),
    Business(U),
}
