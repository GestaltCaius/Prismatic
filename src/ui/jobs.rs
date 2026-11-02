//! Background jobs the page polls for: one job per button press, one item per mod.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use anyhow::Result;
use prismatic::progress::{Event, Progress};
use serde::Serialize;

/// How many finished jobs the page keeps showing
const JOBS_KEPT: usize = 20;

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Skipped,
}

#[derive(Serialize, Clone)]
pub struct Item {
    label: String,
    status: Status,
    message: String,
    stage: String,
    downloaded: u64,
    total: Option<u64>,
}

#[derive(Serialize, Clone)]
pub struct Job {
    id: usize,
    title: String,
    items: Vec<Item>,
    finished: bool,
}

#[derive(Default)]
pub struct Jobs(Mutex<Vec<Job>>);

impl Jobs {
    fn lock(&self) -> MutexGuard<'_, Vec<Job>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn snapshot(&self) -> Vec<Job> {
        self.lock().clone()
    }

    /// Adds a job with all its items queued, returns its id
    pub fn start(&self, title: &str, labels: Vec<String>) -> usize {
        let mut jobs = self.lock();
        let id = jobs.last().map_or(0, |job| job.id + 1);
        jobs.push(Job {
            id,
            title: title.to_string(),
            items: labels.into_iter().map(|label| Item {
                label,
                status: Status::Queued,
                message: String::new(),
                stage: String::new(),
                downloaded: 0,
                total: None,
            }).collect(),
            finished: false,
        });
        while jobs.len() > JOBS_KEPT {
            jobs.remove(0);
        }
        drop(jobs);
        id
    }

    fn with_item(&self, job: usize, index: usize, change: impl FnOnce(&mut Item)) {
        if let Some(item) = self.lock().iter_mut().find(|j| j.id == job).and_then(|j| j.items.get_mut(index)) {
            change(item);
        }
    }

    pub fn running(&self, job: usize, index: usize) {
        self.with_item(job, index, |item| item.status = Status::Running);
    }

    pub fn finish(&self, job: usize, index: usize, status: Status, message: String) {
        self.with_item(job, index, |item| {
            item.status = status;
            item.message = message;
            item.stage.clear();
            item.total = None;
        });
    }

    /// Done with `message` on success, failed with the error otherwise
    pub fn finish_with(&self, job: usize, index: usize, result: Result<String>) {
        match result {
            Ok(message) => self.finish(job, index, Status::Done, message),
            Err(e) => self.finish(job, index, Status::Failed, e.to_string()),
        }
    }

    pub fn complete(&self, job: usize) {
        if let Some(job) = self.lock().iter_mut().find(|j| j.id == job) {
            job.finished = true;
        }
    }

    /// Progress reports that land on one item of a job
    pub fn progress(self: &Arc<Self>, job: usize, index: usize) -> Progress {
        let jobs = self.clone();
        Progress::new(move |event| jobs.with_item(job, index, |item| match event {
            Event::Stage(label) => {
                label.clone_into(&mut item.stage);
                item.downloaded = 0;
                item.total = None;
            }
            Event::Download { done, total } => {
                "Downloading".clone_into(&mut item.stage);
                item.downloaded = done;
                item.total = total;
            }
            Event::Done => item.stage.clear(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use prismatic::progress::Event;
    use super::{Jobs, Status, JOBS_KEPT};

    fn json(jobs: &Jobs) -> serde_json::Value {
        serde_json::to_value(jobs.snapshot()).unwrap()
    }

    #[test]
    fn an_item_goes_from_queued_to_running_to_done() {
        let jobs = Jobs::default();
        let id = jobs.start("Update", vec!["A".into(), "B".into()]);
        assert_eq!(json(&jobs)[0]["items"][0]["status"], "queued");

        jobs.running(id, 0);
        jobs.finish(id, 0, Status::Done, "1 -> 2".into());
        jobs.finish_with(id, 1, Err(anyhow::anyhow!("boom")));
        jobs.complete(id);

        let state = json(&jobs);
        assert_eq!(state[0]["items"][0]["message"], "1 -> 2");
        assert_eq!((state[0]["items"][1]["status"].as_str(), state[0]["items"][1]["message"].as_str()), (Some("failed"), Some("boom")));
        assert_eq!(state[0]["finished"], true);
    }

    #[test]
    fn progress_events_show_up_on_their_item_and_clear_when_done() {
        let jobs = Arc::new(Jobs::default());
        let id = jobs.start("Add", vec!["A".into()]);
        let progress = jobs.progress(id, 0);

        progress.emit(Event::Download { done: 5, total: Some(10) });
        let state = json(&jobs);
        let item = &state[0]["items"][0];
        assert_eq!((item["stage"].as_str(), item["downloaded"].as_u64(), item["total"].as_u64()), (Some("Downloading"), Some(5), Some(10)));

        progress.emit(Event::Done);
        assert_eq!(json(&jobs)[0]["items"][0]["stage"], "");
    }

    #[test]
    fn only_the_newest_jobs_are_kept_and_ids_keep_counting() {
        let jobs = Jobs::default();
        for _ in 0..JOBS_KEPT + 5 {
            jobs.start("Job", vec![]);
        }
        let kept = jobs.snapshot();
        assert_eq!(kept.len(), JOBS_KEPT);
        assert_eq!(json(&jobs)[JOBS_KEPT - 1]["id"], JOBS_KEPT + 4);
    }
}
