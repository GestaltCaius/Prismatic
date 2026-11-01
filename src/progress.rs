use std::sync::{Arc, Mutex};
use std::time::Duration;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

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

fn style(template: &str) -> ProgressStyle {
    ProgressStyle::with_template(template).unwrap_or_else(|_| ProgressStyle::default_bar())
}

/// Shows progress as a bar in `multi`. The bar only appears with the first event, so mods that
/// need no work stay invisible. Bars are hidden when stderr is not a terminal.
pub fn bar(multi: &MultiProgress, label: &str) -> Progress {
    let multi = multi.clone();
    let label = label.to_string();
    let slot: Mutex<Option<ProgressBar>> = Mutex::new(None);
    Progress(Arc::new(move |event| {
        let Ok(mut slot) = slot.lock() else { return };
        if matches!(event, Event::Done) {
            if let Some(bar) = slot.take() {
                bar.finish_and_clear();
            }
            return;
        }
        let bar = slot.get_or_insert_with(|| {
            let bar = multi.add(ProgressBar::new_spinner().with_prefix(label.clone()));
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });
        match event {
            Event::Stage(stage) => {
                bar.set_style(style("{prefix:.bold} {spinner} {msg}"));
                bar.unset_length();
                bar.set_message(stage);
            }
            Event::Download { done, total } => {
                if let Some(total) = total {
                    if bar.length() != Some(total) {
                        bar.set_length(total);
                        bar.set_style(style("{prefix:.bold} {msg} {wide_bar} {bytes}/{total_bytes}"));
                    }
                }
                bar.set_message("Downloading");
                bar.set_position(done);
            }
            Event::Done => {}
        }
    }))
}
