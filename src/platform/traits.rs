use anyhow::Result;
use std::path::{Path, PathBuf};

/// Path normalization for platform-specific requirements
pub trait PathNormalizer {
    /// Normalize path for the platform (e.g., Windows long path support)
    fn normalize(&self, path: &Path) -> PathBuf;
}

/// File system operations abstraction
pub trait FileSystem {
    /// Copy file with platform-specific optimizations(not yet, but planned)
    fn copy_file(&self, src: &Path, dst: &Path) -> impl Future<Output=Result<u64>> + Send;
}