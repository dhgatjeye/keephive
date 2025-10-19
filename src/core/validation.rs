use anyhow::{bail, Context, Result};
use std::path::Path;
use tracing::{debug, warn};

#[derive(Debug)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub warnings: Vec<String>,
}

pub async fn validate_backup_job(source: &Path, target: &Path) -> Result<ValidationResult> {
    let mut warnings = Vec::new();

    debug!("Validating backup job: {:?} -> {:?}", source, target);

    // 1. Source exists and is readable
    if !source.exists() {
        bail!("Source path does not exist: {}", source.display());
    }

    if !source.is_dir() {
        bail!("Source path is not a directory: {}", source.display());
    }

    if source == target {
        bail!("Source and target directories cannot be the same");
    }

    // 2. Test read access on source
    match tokio::fs::read_dir(source).await {
        Ok(_) => debug!("Source is readable"),
        Err(e) => bail!("Cannot read source directory: {}", e),
    }

    // 3. Target directory checks
    if !target.exists() {
        // Try to create target directory
        tokio::fs::create_dir_all(target).await
            .context("Cannot create target directory")?;
        debug!("Created target directory: {}", target.display());
    } else if !target.is_dir() {
        bail!("Target path exists but is not a directory: {}", target.display());
    }

    // 4. Test write access on target
    let test_file = target.join(".keephive_write_test");
    match tokio::fs::write(&test_file, b"test").await {
        Ok(_) => {
            let _ = tokio::fs::remove_file(&test_file).await;
            debug!("Target is writable");
        }
        Err(e) => bail!("Cannot write to target directory: {}", e),
    }

    // 5. Check for circular paths (target inside source)
    if target.starts_with(source) {
        bail!("Target directory cannot be inside source directory");
    }

    // 6. Check available disk space
    match check_disk_space(source, target).await {
        Ok(true) => debug!("Sufficient disk space available"),
        Ok(false) => warnings.push("Target disk space may be insufficient".to_string()),
        Err(e) => {
            warn!("Could not check disk space: {}", e);
            warnings.push(format!("Could not verify disk space: {}", e));
        }
    }

    // 7. Path length validation (Windows long path awareness)
    #[cfg(windows)]
    if source.as_os_str().len() > 200 || target.as_os_str().len() > 200 {
        debug!("Long paths detected - will use Windows extended path prefix");
        warnings.push("Using Windows extended path support for long paths".to_string());
    }

    Ok(ValidationResult {
        is_valid: true,
        warnings,
    })
}

async fn check_disk_space(source: &Path, target: &Path) -> Result<bool> {
    let source_size = calculate_dir_size(source).await?;

    // Get available space on target drive
    #[cfg(windows)]
    {
        use crate::platform::windows::file_ops::get_disk_free_space;
        let available = get_disk_free_space(target)?;
        let required = source_size.saturating_mul(11) / 10;
        Ok(available >= required)
    }

    #[cfg(not(windows))]
    {
        // For future cross-platform support
        warn!("Disk space check not implemented for this platform");
        Ok(true)
    }
}

/// Calculate total size of directory
async fn calculate_dir_size(path: &Path) -> Result<u64> {
    let mut total_size = 0u64;
    let mut stack = vec![path.to_path_buf()];

    while let Some(current) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&current).await {
            Ok(e) => e,
            Err(_) => continue, // Skip inaccessible directories
        };

        while let Some(entry) = entries.next_entry().await? {
            let metadata = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue, // Skip inaccessible files
            };

            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total_size += metadata.len();
            }
        }
    }

    Ok(total_size)
}