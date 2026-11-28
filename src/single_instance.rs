//! Ensures only one Hermit runs per user.
//!
//! The running instance holds an exclusive lock on a file. The operating system
//! releases the lock when the process exits, even after a crash, so a stale lock
//! file never blocks the next start. The file holds the running instance's
//! process id, so a development build can stop it and take its place.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Seek as _, Write as _};
use std::path::PathBuf;

use anyhow::Context as _;

use crate::telemetry::private_path;

/// Held for as long as this process is the running instance.
#[must_use = "the lock is released when this is dropped"]
pub struct InstanceLock {
    _file: File,
}

/// Takes the instance lock. When another instance holds it, `replace_running`
/// stops that instance and takes over (on Linux); otherwise this returns `None`.
pub fn acquire(replace_running: bool) -> anyhow::Result<Option<InstanceLock>> {
    let path = lock_path();
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", private_path(&path)))?;

    if !try_lock(&file).with_context(|| format!("locking {}", private_path(&path)))? {
        if !replace_running {
            return Ok(None);
        }
        if let Err(error) = replace(&mut file) {
            tracing::warn!("could not replace the running Hermit: {error:#}");
            return Ok(None);
        }
    }

    record_process_id(&mut file).with_context(|| format!("writing {}", private_path(&path)))?;
    Ok(Some(InstanceLock { _file: file }))
}

/// Returns whether the lock was taken; `false` when another process holds it.
fn try_lock(file: &File) -> std::io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn record_process_id(file: &mut File) -> std::io::Result<()> {
    file.set_len(0)?;
    file.rewind()?;
    write!(file, "{}", std::process::id())?;
    file.flush()
}

/// Stops the instance holding the lock and waits until its lock is released.
#[cfg(target_os = "linux")]
fn replace(file: &mut File) -> anyhow::Result<()> {
    use std::io::Read as _;
    use std::time::{Duration, Instant};

    use rustix::process::{Pid, Signal, kill_process};

    /// Long enough for an instance to exit, short enough not to stall a start.
    const EXIT_TIMEOUT: Duration = Duration::from_secs(3);

    let mut contents = String::new();
    file.rewind()?;
    file.read_to_string(&mut contents)?;
    let pid = contents
        .trim()
        .parse()
        .ok()
        .and_then(Pid::from_raw)
        .context("the running instance did not record its process id")?;

    tracing::info!(
        pid = pid.as_raw_nonzero().get(),
        "stopping the running Hermit"
    );
    kill_process(pid, Signal::TERM).context("sending it SIGTERM")?;
    // A suspended instance (Ctrl+Z in its terminal) acts on SIGTERM only once resumed.
    kill_process(pid, Signal::CONT).context("sending it SIGCONT")?;

    let deadline = Instant::now() + EXIT_TIMEOUT;
    while !try_lock(file)? {
        if Instant::now() >= deadline {
            anyhow::bail!("it did not exit within {EXIT_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn replace(_file: &mut File) -> anyhow::Result<()> {
    anyhow::bail!("replacing the running instance is only supported on Linux")
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
