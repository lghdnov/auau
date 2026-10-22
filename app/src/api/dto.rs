use serde::Serialize;

use crate::models::{FileStatus, Label, Project};

/// Per-status counters for a project's audio files.
///
/// `processing` files are mid-transcode: surfaced as their own counter so
/// the dashboard can show them, but never claimable.
#[derive(Debug, Default, Serialize)]
pub struct FileCounts {
    pub new: i64,
    pub in_progress: i64,
    pub done: i64,
    pub error: i64,
    pub processing: i64,
}

impl FileCounts {
    /// Fold one `(status, n)` group from a `GROUP BY status` query into
    /// the counters.
    pub(crate) fn add(&mut self, status: FileStatus, count: i64) {
        match status {
            FileStatus::New => self.new += count,
            FileStatus::InProgress => self.in_progress += count,
            FileStatus::Done => self.done += count,
            FileStatus::Error => self.error += count,
            FileStatus::Processing => self.processing += count,
        }
    }
}

/// A project as listed on the dashboard: the row plus file counters.
#[derive(Debug, Serialize)]
pub struct ProjectSummary {
    pub project: Project,
    pub counts: FileCounts,
}

/// A single project with its live label palette and file counters.
#[derive(Debug, Serialize)]
pub struct ProjectDetail {
    pub project: Project,
    pub labels: Vec<Label>,
    pub counts: FileCounts,
}
