pub struct BackgroundTasks {
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl BackgroundTasks {
    pub fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    pub fn spawn(&mut self, handle: tokio::task::JoinHandle<()>) {
        self.handles.push(handle);
    }

    pub fn abort_all(self) {
        for handle in self.handles {
            handle.abort();
        }
    }
}

impl Default for BackgroundTasks {
    fn default() -> Self {
        Self::new()
    }
}
