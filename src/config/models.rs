use chrono::Duration;
use chrono::{Datelike, Local, Timelike};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Default number of backups to retain per job
pub const DEFAULT_RETENTION_COUNT: usize = 5;
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_STATE_FILE: &str = ".keephive_state.json";

#[inline]
fn default_retention_count() -> usize {
    DEFAULT_RETENTION_COUNT
}

#[inline]
fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}

#[inline]
fn default_state_path() -> PathBuf {
    PathBuf::from(DEFAULT_STATE_FILE)
}

/// Main service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// List of backup jobs
    pub jobs: Vec<BackupJob>,

    /// Maximum number of backups to retain per job
    #[serde(default = "default_retention_count")]
    pub retention_count: usize,

    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// State file path
    #[serde(default = "default_state_path")]
    pub state_path: PathBuf,

    /// Optional log file directory (if None, only console logging)
    #[serde(default)]
    pub log_directory: Option<PathBuf>,

    /// Log file rotation strategy
    #[serde(default)]
    pub log_rotation: LogRotation,
}


/// Log file rotation strategy
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LogRotation {
    /// Rotate daily
    Daily,
    /// Rotate hourly
    Hourly,
    /// Never rotate (single file)
    Never,
}

impl Default for LogRotation {
    fn default() -> Self {
        LogRotation::Daily
    }
}

/// Individual backup job configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupJob {
    /// Unique job identifier
    pub id: String,

    /// Source directory to backup
    pub source: PathBuf,

    /// Target directory for backups
    pub target: PathBuf,

    /// Backup schedule
    pub schedule: Schedule,

    /// Optional description
    #[serde(default)]
    pub description: String,
}

/// Backup schedule configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Schedule {
    /// Run at specific interval
    Interval {
        /// Duration in seconds
        seconds: u64,
    },

    /// Daily at specific time
    Daily {
        /// Hour (0-23)
        hour: u32,
        /// Minute (0-59)
        minute: u32,
    },

    /// Weekly on specific day
    Weekly {
        /// Day of week (1=Monday, 7=Sunday)
        day: u32,
        /// Hour (0-23)
        hour: u32,
        /// Minute (0-59)
        minute: u32,
    },
}

impl Schedule {
    /// Get duration until next run from now
    pub fn next_run_duration(&self, last_run: Option<chrono::DateTime<chrono::Utc>>) -> Duration {
        match self {
            Schedule::Interval { seconds } => {
                if let Some(last) = last_run {
                    let elapsed = Local::now().signed_duration_since(last);
                    let interval = Duration::seconds(*seconds as i64);
                    if elapsed >= interval {
                        Duration::zero()
                    } else {
                        interval - elapsed
                    }
                } else {
                    Duration::zero()
                }
            }
            Schedule::Daily { hour, minute } => {
                Self::calculate_next_daily(*hour, *minute, last_run)
            }
            Schedule::Weekly { day, hour, minute } => {
                Self::calculate_next_weekly(*day, *hour, *minute, last_run)
            }
        }
    }

    fn calculate_next_daily(hour: u32, minute: u32, _last_run: Option<chrono::DateTime<chrono::Utc>>) -> Duration {
        let now = Local::now();
        let today_scheduled = now
            .date_naive()
            .and_hms_opt(hour, minute, 0)
            .unwrap();

        let next = if now.time() < today_scheduled.time() {
            today_scheduled
        } else {
            (now.date_naive() + Duration::days(1))
                .and_hms_opt(hour, minute, 0)
                .unwrap()
        };

        let next_datetime = next.and_local_timezone(now.timezone()).unwrap();
        next_datetime.signed_duration_since(now)
    }

    fn calculate_next_weekly(day: u32, hour: u32, minute: u32, _last_run: Option<chrono::DateTime<chrono::Utc>>) -> Duration {
        let now = Local::now();
        let current_weekday = now.weekday().num_days_from_monday() + 1; // 1=Monday, 7=Sunday

        // Calculate days until target weekday
        let days_until = if current_weekday < day {
            // Target day is later this week
            day - current_weekday
        } else if current_weekday == day {
            // Today is the target day - check if time has passed
            let target_time_passed = now.hour() > hour
                || (now.hour() == hour && now.minute() >= minute);

            if target_time_passed {
                // Time has passed today, schedule for next week
                7
            } else {
                // Time hasn't passed yet today
                0
            }
        } else {
            // Target day was earlier this week, schedule for next week
            // current_weekday > day
            7 - (current_weekday - day)
        };

        let next_date = now.date_naive() + Duration::days(days_until as i64);
        let next_datetime = next_date
            .and_hms_opt(hour, minute, 0)
            .expect("Invalid hour/minute for weekly schedule");

        let next = next_datetime.and_local_timezone(now.timezone()).unwrap();
        next.signed_duration_since(now)
    }
}

/// Backup configuration (alias for compatibility)
pub type BackupConfig = ServiceConfig;