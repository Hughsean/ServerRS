pub mod tools {
    pub use ai_core::tool::*;
}

pub use ai_core::chat::*;

pub trait PromptProvider: Send + Sync {
    fn get_prompt(&self, date_time: &str) -> String;
}
