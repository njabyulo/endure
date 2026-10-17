//! Process-scoped leases prevent two hosts from executing the same durable run.

use std::{
    fs::{self, File, OpenOptions, TryLockError},
    path::Path,
};

use sha2::{Digest, Sha256};

/// The operating system releases the lock when this handle or its process exits.
/// Lock files remain in place so another process cannot acquire a replacement inode.
pub(crate) struct RunLease {
    _file: File,
}

impl RunLease {
    pub(crate) fn acquire(journal_path: &Path, run_id: &str) -> Result<Self, String> {
        let canonical = if journal_path.exists() {
            journal_path.canonicalize()
        } else {
            let parent = journal_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            parent
                .canonicalize()
                .map(|parent| parent.join(journal_path.file_name().unwrap_or_default()))
        }
        .map_err(|error| format!("run lease: {error}"))?;
        let mut directory = canonical.into_os_string();
        directory.push(".leases");
        let directory = std::path::PathBuf::from(directory);
        fs::create_dir_all(&directory).map_err(|error| format!("run lease: {error}"))?;
        let name = format!("{:x}", Sha256::digest(run_id.as_bytes()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(name))
            .map_err(|error| format!("run lease: {error}"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(format!("run {run_id} is already running")),
            Err(TryLockError::Error(error)) => Err(format!("run lease: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_are_exclusive_and_released_when_dropped() {
        let directory = std::env::temp_dir().join(format!("ondure-lease-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let journal = directory.join("journal.sqlite");
        let lease = RunLease::acquire(&journal, "one").unwrap();
        assert!(RunLease::acquire(&journal, "one")
            .err()
            .unwrap()
            .contains("already running"));
        let other = RunLease::acquire(&journal, "two").unwrap();
        drop(lease);
        let replacement = RunLease::acquire(&journal, "one").unwrap();
        drop((other, replacement));
        fs::remove_dir_all(directory).unwrap();
    }
}
