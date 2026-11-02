use std::sync::Arc;

pub enum Event {
    Stage(&'static str),
    Download { done: u64, total: Option<u64> },
    /// The operation is over, successfully or not
    Done,
}

/// Where the download and install code reports what it is doing, so each front end can show it its own way
#[derive(Clone)]
pub struct Progress(Arc<dyn Fn(Event) + Send + Sync>);

impl Progress {
    pub fn new(report: impl Fn(Event) + Send + Sync + 'static) -> Self {
        Self(Arc::new(report))
    }

    pub fn emit(&self, event: Event) {
        (self.0)(event);
    }
}
