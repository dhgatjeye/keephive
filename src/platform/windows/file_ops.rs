use anyhow::{Context, Result};
use std::path::Path;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::debug;

/// Buffer size for streaming copy (1MB)
const COPY_BUFFER_SIZE: usize = 1024 * 1024;

pub async fn copy_file(src: &Path, dst: &Path) -> Result<u64> {
    debug!("Copying file: {:?} -> {:?}", src, dst);

    let mut src_file = tokio::fs::File::open(src).await
        .context("Failed to open source file")?;

    let mut dst_file = tokio::fs::File::create(dst).await
        .context("Failed to create destination file")?;

    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    let mut total_bytes = 0u64;

    loop {
        let bytes_read = src_file.read(&mut buffer).await
            .context("Failed to read from source")?;

        if bytes_read == 0 {
            break;
        }

        dst_file.write_all(&buffer[..bytes_read]).await
            .context("Failed to write to destination")?;

        total_bytes += bytes_read as u64;
    }

    // Sync destination file
    dst_file.sync_all().await
        .context("Failed to sync destination file")?;

    // Copy metadata (timestamps)
    copy_metadata(src, dst).await?;

    Ok(total_bytes)
}

async fn copy_metadata(src: &Path, dst: &Path) -> Result<()> {
    let metadata = tokio::fs::metadata(src).await?;

    // Set modification time
    #[cfg(windows)]
    {
        use std::fs::OpenOptions;
        let file = OpenOptions::new()
            .write(true)
            .open(dst)?;

        file.set_modified(metadata.modified()?)?;
    }

    Ok(())
}

#[cfg(windows)]
pub fn get_disk_free_space(path: &Path) -> Result<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;
    use std::os::windows::ffi::OsStrExt;
    use anyhow::bail;

    // Get the root path (drive letter)
    let root = if let Some(prefix) = path.components().next() {
        PathBuf::from(prefix.as_os_str()).join("\\")
    } else {
        bail!("Invalid path: path has no components and cannot determine disk space");
    };

    let root_wide: Vec<u16> = root.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut free_bytes_available = 0u64;
    let mut _total_bytes = 0u64;
    let mut _total_free_bytes = 0u64;

    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(root_wide.as_ptr()),
            Some(&mut free_bytes_available as *mut u64),
            Some(&mut _total_bytes as *mut u64),
            Some(&mut _total_free_bytes as *mut u64),
        )?;
    }

    Ok(free_bytes_available)
}