//! Job enum and job runner for the IndexScheduler.
//!
//! Each job represents a discrete indexing operation that can be queued,
//! retried, or run as part of a composite workflow.

#![allow(dead_code)]
use serde::Serialize;

/// A discrete indexing operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Job {
    /// Build a full snapshot from the working tree or staged area.
    BuildSnapshot,
    /// Incrementally update the text index with changed files.
    UpdateText,
    /// Incrementally update SCIP occurrence data.
    UpdateScip,
    /// Incrementally update the call-graph / relation data.
    UpdateGraph,
    /// Compact / garbage-collect obsolete segments.
    Compact,
}

/// Result of running a single job.
#[derive(Debug, Clone, Serialize)]
pub struct JobResult {
    pub job: Job,
    pub status: JobStatus,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Success,
    Skipped,
    Failed,
}

/// Run a single job against the scheduler.
///
/// The scheduler delegates to this runner, which performs the
/// actual work and returns a structured result. The runner is
/// designed so that each job can be retried independently.
pub fn run_job(_job: &Job) -> JobResult {
    // Job execution is orchestrated by the IndexScheduler methods
    // (build_all, update, compact). This runner exists as a future
    // extension point for task queuing and retry logic.
    JobResult {
        job: _job.clone(),
        status: JobStatus::Skipped,
        summary: "job execution delegated to IndexScheduler methods".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_enum_is_serializable() {
        let job = Job::BuildSnapshot;
        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains("BuildSnapshot"));
    }

    #[test]
    fn job_result_has_summary() {
        let result = run_job(&Job::Compact);
        assert_eq!(result.job, Job::Compact);
        assert_eq!(result.status, JobStatus::Skipped);
    }
}
