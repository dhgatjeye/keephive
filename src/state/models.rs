use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Current state schema version for migrations
pub const STATE_SCHEMA_VERSION: u32 = 1;

/// Root state structure persisted to disk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupState {
    /// Schema version for future migrations
    pub version: u32,

    /// All job states
    pub jobs: Vec<JobState>,

    /// Last time state was updated
    pub last_updated: DateTime<Utc>,
}

impl BackupState {
    /// Create a new empty backup state with current timestamp
    pub fn new() -> Self {
        Self {
            version: STATE_SCHEMA_VERSION,
            jobs: Vec::new(),
            last_updated: Utc::now(),
        }
    }

    /// Update or insert job state
    pub fn upsert_job(&mut self, job: JobState) {
        if let Some(existing) = self.jobs.iter_mut().find(|j| j.id == job.id) {
            *existing = job;
        } else {
            self.jobs.push(job);
        }
        self.last_updated = Utc::now();
    }

    /// Get job state by ID
    pub fn get_job(&self, id: &str) -> Option<&JobState> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Get mutable job state by ID
    pub fn get_job_mut(&mut self, id: &str) -> Option<&mut JobState> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }
}

/// Job execution status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum JobStatus {
    /// Waiting for next scheduled run
    Idle,

    /// Currently running
    Running {
        started_at: DateTime<Utc>,
    },

    /// Failed
    Failed {
        error: String,
        timestamp: DateTime<Utc>,
    },
}

/// State of an individual backup job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobState {
    /// Job identifier (matches config)
    pub id: String,

    /// Current source path (for detecting config changes)
    pub source: PathBuf,

    /// Current target path (for detecting config changes)
    pub target: PathBuf,

    /// Current job status
    pub status: JobStatus,

    /// Last successful backup timestamp
    pub last_run: Option<DateTime<Utc>>,

    /// Next scheduled run
    pub next_run: Option<DateTime<Utc>>,

    /// Metadata from last backup
    pub last_backup: Option<BackupMetadata>,

    /// Active backup metadata (if currently running)
    pub active_backup: Option<BackupMetadata>,
}

impl JobState {
    pub fn new(id: String, source: PathBuf, target: PathBuf) -> Self {
        Self {
            id,
            source,
            target,
            status: JobStatus::Idle,
            last_run: None,
            next_run: None,
            last_backup: None,
            active_backup: None,
        }
    }
}

/// Metadata about a backup
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Backup directory name
    pub backup_name: String,

    /// Full path to backup
    pub backup_path: PathBuf,

    /// Start timestamp
    pub started_at: DateTime<Utc>,

    /// Completion timestamp (None if partial/in-progress)
    pub completed_at: Option<DateTime<Utc>>,

    /// Total bytes copied
    pub bytes_copied: u64,

    /// Total files copied
    pub files_copied: u64,

    /// Total files skipped (e.g., locked files)
    pub files_skipped: u64,

    /// Whether backup completed successfully
    pub is_complete: bool,

    /// Errors encountered (non-fatal)
    pub errors: Vec<String>,
}

impl BackupMetadata {
    /// Create new backup metadata with current timestamp
    pub fn new(backup_name: String, backup_path: PathBuf) -> Self {
        Self {
            backup_name,
            backup_path,
            started_at: Utc::now(),
            completed_at: None,
            bytes_copied: 0,
            files_copied: 0,
            files_skipped: 0,
            is_complete: false,
            errors: Vec::new(),
        }
    }

    pub fn mark_complete(&mut self) {
        self.completed_at = Some(Utc::now());
        self.is_complete = true;
    }
}