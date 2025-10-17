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

        // Already has extended prefix
        if path_str.starts_with(EXTENDED_PATH_PREFIX) {
            return path.to_path_buf();
        }

        // Check if path exceeds windows limit or will likely exceed during operations
        if path_str.len() > WINDOWS_MAX_PATH - 50 {
            // Convert to absolute path first
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                match std::env::current_dir() {
                    Ok(cwd) => cwd.join(path),
                    Err(e) => {
                        tracing::error!("Failed to get current dir for normalization: {}", e);
                        return path.to_path_buf();
                    }
                }
            };

            let extended = if absolute.to_string_lossy().starts_with(r"\\") {
                format!("\\\\?\\UNC{}", &absolute.to_string_lossy()[1..])
            } else {
                format!("{}{}", EXTENDED_PATH_PREFIX, absolute.display())
            };
            PathBuf::from(extended)
        } else {
            path.to_path_buf()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_path_unchanged() {
        let normalizer = WindowsPathNormalizer;
        let path = Path::new(r"C:\Users\test\file.txt");
        let normalized = normalizer.normalize(path);
        assert_eq!(normalized, path);
    }

    #[test]
    fn test_long_path_gets_prefix() {
        let normalizer = WindowsPathNormalizer;

        let long_path = "C:\\".to_string() + &"verylongdirectoryname\\".repeat(12);
        let path = Path::new(&long_path);
        let normalized = normalizer.normalize(path);

        println!("Original path length: {}", long_path.len());
        println!("Normalized path: {}", normalized.display());

        assert!(normalized.to_string_lossy().starts_with(EXTENDED_PATH_PREFIX));
    }

    #[test]
    fn test_already_prefixed_path_unchanged() {
        let normalizer = WindowsPathNormalizer;
        let prefixed_path = r"\\?\C:\some\long\path";
        let path = Path::new(prefixed_path);
        let normalized = normalizer.normalize(path);
        assert_eq!(normalized.to_string_lossy(), prefixed_path);
    }

    #[test]
    fn test_relative_long_path_becomes_absolute_with_prefix() {
        let normalizer = WindowsPathNormalizer;

        let long_relative = "a\\".to_string() + &"verylongdirectoryname\\".repeat(12);
        let path = Path::new(&long_relative);
        let normalized = normalizer.normalize(path);

        let normalized_str = normalized.to_string_lossy();
        assert!(normalized_str.starts_with(EXTENDED_PATH_PREFIX));

        assert!(normalized_str.contains("\\a\\"));
    }

    #[test]
    fn test_empty_path_returns_unchanged() {
        let normalizer = WindowsPathNormalizer;
        let empty_path = Path::new("");
        let normalized = normalizer.normalize(empty_path);

        // Should return the path as-is (empty) with warning
        assert_eq!(normalized.to_string_lossy(), "");
    }

    #[test]
    fn test_short_relative_path_unchanged() {
        let normalizer = WindowsPathNormalizer;
        let path = Path::new("short\\relative\\path.txt");
        let normalized = normalizer.normalize(path);

        // Short relative paths should not get extended prefix
        assert!(!normalized.to_string_lossy().starts_with(EXTENDED_PATH_PREFIX));
        assert_eq!(normalized, path);
    }

    #[test]
    fn test_absolute_long_path_with_prefix() {
        let normalizer = WindowsPathNormalizer;

        // Create a long absolute path (over 260 chars)
        let long_path = "C:\\".to_string() + &"verylongdirectoryname123456789\\".repeat(9) + "file.txt";
        assert!(long_path.len() > WINDOWS_MAX_PATH);

        let path = Path::new(&long_path);
        let normalized = normalizer.normalize(path);

        let normalized_str = normalized.to_string_lossy();
        assert!(normalized_str.starts_with(EXTENDED_PATH_PREFIX));
        assert!(normalized_str.contains("C:\\"));
    }

    #[test]
    fn test_unc_path_handling() {
        let normalizer = WindowsPathNormalizer;
        let unc_path = r"\\server\share\file.txt";
        let path = Path::new(unc_path);
        let normalized = normalizer.normalize(path);

        // Short UNC paths should remain unchanged
        assert_eq!(normalized.to_string_lossy(), unc_path);
    }

    #[test]
    fn test_path_with_trailing_backslash() {
        let normalizer = WindowsPathNormalizer;
        let path_with_slash = Path::new(r"C:\Users\test\");
        let normalized = normalizer.normalize(path_with_slash);

        // Should handle trailing backslash gracefully
        assert_eq!(normalized, path_with_slash);
    }
}