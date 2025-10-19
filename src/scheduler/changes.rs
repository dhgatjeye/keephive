use crate::config::BackupJob;

/// Configuration change detection result
#[derive(Debug)]
pub struct ConfigChanges {
    pub added: Vec<BackupJob>,
    pub removed: Vec<String>,
    pub modified: Vec<ModifiedJob>,
}

/// Details about a modified job
#[derive(Debug, Clone)]
pub struct ModifiedJob {
    pub job: BackupJob,
    pub change_type: ConfigChangeType,
}

/// Type of configuration change
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigChangeType {
    /// Only schedule changed (safe to continue running job)
    ScheduleOnly,
    /// Source or target changed (requires job restart)
    PathChanged,
    /// Both changed
    PathAndSchedule,
}