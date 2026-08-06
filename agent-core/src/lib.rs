//! Business-neutral Agent Graph runtime, state machine, effects and checkpoints.

mod action;
pub mod graph;
mod message;
mod state;

pub use action::{AgentAction, AgentPolicy};
pub use message::{AgentMessage, AgentObservation, AgentOutcome, AgentToolCall};
pub use state::{
    AgentBusinessState, AgentState, AgentStateError, AgentUpdate, PromptSection, PromptSource,
    PromptTrust, StateSchemaVersion,
};
