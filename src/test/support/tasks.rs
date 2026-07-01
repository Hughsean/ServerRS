use std::sync::Mutex;

use async_trait::async_trait;

use crate::domain::tasks::task_event::{TaskEvent, TurnClosedEvent};
use crate::domain::tasks::task_publisher::TaskPublisher;
use crate::shared::error::AppError;

#[derive(Default)]
pub struct RecordingTaskPublisher {
    events: Mutex<Vec<TaskEvent>>,
}

impl RecordingTaskPublisher {
    pub fn turn_closed_events(&self) -> Vec<TurnClosedEvent> {
        self.events
            .lock()
            .expect("task event lock poisoned")
            .iter()
            .filter_map(|event| match event {
                TaskEvent::TurnClosed(turn) => Some(turn.clone()),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl TaskPublisher for RecordingTaskPublisher {
    async fn publish(&self, event: TaskEvent) -> Result<(), AppError> {
        self.events
            .lock()
            .expect("task event lock poisoned")
            .push(event);
        Ok(())
    }
}
