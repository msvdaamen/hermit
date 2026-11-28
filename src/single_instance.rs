//! Ensures only one Hermit runs per user.
//!
//! The running instance holds an exclusive lock on a file. The operating system
//! releases the lock when the process exits, even after a crash, so a stale lock
//! file never blocks the next start.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;

use anyhow::Context as _;

use crate::telemetry::private_path;

/// Held for as long as this process is the running instance.
#[must_use = "the lock is released when this is dropped"]
pub struct InstanceLock {
    _file: File,
}

/// Takes the instance lock, or returns `None` when another instance holds it.
pub fn acquire() -> anyhow::Result<Option<InstanceLock>> {
    let path = lock_path();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", private_path(&path)))?;

    match file.try_lock() {
        Ok(()) => Ok(Some(InstanceLock { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("locking {}", private_path(&path)))
        }
    }
}

/// A per-user location: the session runtime directory on Linux, the user's
/// temporary directory on macOS and Windows.
fn lock_path() -> PathBuf {
    let file_name = format!("{}.lock", crate::APP_ID);

    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(runtime_dir).join(file_name);
    }

    std::env::temp_dir().join(file_name)
}
