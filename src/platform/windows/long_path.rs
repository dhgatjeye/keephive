use crate::platform::traits::PathNormalizer;
use std::path::{Path, PathBuf};

/// Windows long path limit
const WINDOWS_MAX_PATH: usize = 260;

/// Windows extended path prefix
const EXTENDED_PATH_PREFIX: &str = r"\\?\";

pub struct WindowsPathNormalizer;

impl PathNormalizer for WindowsPathNormalizer {
    fn normalize(&self, path: &Path) -> PathBuf {
        let path_str = path.to_string_lossy();

        // Already has extended prefix, keep it
        if path_str.starts_with(EXTENDED_PATH_PREFIX) {
            return path.to_path_buf();
        }

        // Try to canonicalize (only works for existing paths)
        match dunce::canonicalize(path) {
            Ok(normalized) => {
                let normalized_str = normalized.to_string_lossy();

                // Only add prefix if path is actually long
                if normalized_str.len() > WINDOWS_MAX_PATH - 50 {
                    tracing::debug!(
                        "Path exceeds MAX_PATH ({}), adding extended prefix",
                        normalized_str.len()
                    );

                    if normalized_str.starts_with(r"\\") {
                        PathBuf::from(format!(r"\\?\UNC{}", &normalized_str[1..]))
                    } else {
                        PathBuf::from(format!(r"{}{}", EXTENDED_PATH_PREFIX, normalized_str))
                    }
                } else {
                    // Short path, no prefix needed
                    normalized
                }
            }
            Err(e) => {
                // Path doesn't exist or can't be accessed
                tracing::debug!(
                    "Cannot canonicalize '{}': {}. Using original path.",
                    path.display(),
                    e
                );
                path.to_path_buf()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_existing_short_path() {
        let normalizer = WindowsPathNormalizer;
        let temp = std::env::temp_dir();

        let normalized = normalizer.normalize(&temp);

        assert!(normalized.exists());
        assert!(!normalized.to_string_lossy().starts_with(EXTENDED_PATH_PREFIX));
    }

    #[test]
    fn test_normalize_existing_long_path() {
        let normalizer = WindowsPathNormalizer;
        let temp = std::env::temp_dir();

        // Create deep directory structure
        let mut deep_path = temp.clone();
        for i in 0..20 {
            deep_path.push(format!("verylongdirectoryname_{}", i));
        }

        // Create it
        std::fs::create_dir_all(&deep_path).ok();

        if deep_path.to_string_lossy().len() > WINDOWS_MAX_PATH {
            let normalized = normalizer.normalize(&deep_path);

            // Should have prefix for long existing paths
            assert!(normalized.to_string_lossy().starts_with(EXTENDED_PATH_PREFIX));
        }

        // Cleanup
        std::fs::remove_dir_all(temp.join("verylongdirectoryname_0")).ok();
    }

    #[test]
    fn test_normalize_nonexistent_returns_original() {
        let normalizer = WindowsPathNormalizer;
        let fake = Path::new("C:\\this\\does\\not\\exist");

        let normalized = normalizer.normalize(fake);

        // Should return original path unchanged
        assert_eq!(normalized, fake);
    }

    #[test]
    fn test_normalize_already_has_prefix() {
        let normalizer = WindowsPathNormalizer;
        let with_prefix = Path::new(r"\\?\C:\test\path");

        let normalized = normalizer.normalize(with_prefix);

        // Should keep the prefix
        assert_eq!(normalized, with_prefix);
    }
}