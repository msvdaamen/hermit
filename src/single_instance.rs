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

    // Only replacing needs the process id, so the lock is still held without it.
    if let Err(error) = record_process_id(&mut file) {
        tracing::warn!(
            "could not record the process id in {}: {error}",
            private_path(&path)
        );
    }
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

/// The process id the running instance recorded, if the file holds a valid one.
#[cfg(any(target_os = "linux", test))]
fn recorded_process_id(file: &mut File) -> std::io::Result<Option<u32>> {
    use std::io::Read as _;

    let mut contents = String::new();
    file.rewind()?;
    file.read_to_string(&mut contents)?;
    Ok(contents.trim().parse().ok().filter(|&pid| pid != 0))
}

/// Stops the instance holding the lock and waits until its lock is released.
#[cfg(target_os = "linux")]
fn replace(file: &mut File) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};

    use rustix::process::{Pid, Signal, kill_process};

    /// Long enough for an instance to flush its telemetry and exit (see
    /// `telemetry::SHUTDOWN_TIMEOUT`), short enough not to stall a start.
    const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

    let pid = recorded_process_id(file)?
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(Pid::from_raw)
        .context("the running instance did not record its process id")?;
    let raw_pid = pid.as_raw_nonzero().get();

    // The id may be stale: an older Hermit that records none can hold the lock
    // while the file still names an earlier instance, whose id may since have
    // gone to an unrelated process.
    if !is_hermit(raw_pid).with_context(|| format!("checking process {raw_pid}"))? {
        anyhow::bail!("process {raw_pid}, recorded as the running instance, is not Hermit");
    }

    tracing::info!(pid = raw_pid, "stopping the running Hermit");
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

/// Whether the process runs an executable with this one's file name, so a
/// development build also replaces an installed Hermit.
#[cfg(target_os = "linux")]
fn is_hermit(pid: i32) -> std::io::Result<bool> {
    let theirs = std::fs::read_link(format!("/proc/{pid}/exe"))?;
    let ours = std::env::current_exe()?;
    Ok(executable_name(&theirs).is_some_and(|name| Some(name) == executable_name(&ours)))
}

/// The executable's file name. Linux marks the link of a running executable that
/// was since replaced on disk (as a rebuild does) with ` (deleted)`.
#[cfg(target_os = "linux")]
fn executable_name(path: &std::path::Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    Some(name.strip_suffix(" (deleted)").unwrap_or(name))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock file of its own for each test, removed when dropped.
    struct TestFile(PathBuf);

    impl TestFile {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("hermit-test-{}-{name}.lock", std::process::id()));
            Self(path)
        }

        fn open(&self) -> File {
            OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&self.0)
                .unwrap()
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).ok();
        }
    }

    #[test]
    fn reads_back_the_recorded_process_id() {
        let test_file = TestFile::new("record");
        let mut file = test_file.open();
        // A longer id left by an earlier instance must not leave digits behind.
        write!(file, "123456789").unwrap();

        record_process_id(&mut file).unwrap();

        let mut reader = test_file.open();
        assert_eq!(
            recorded_process_id(&mut reader).unwrap(),
            Some(std::process::id())
        );
    }

    #[test]
    fn rejects_missing_and_invalid_process_ids() {
        let test_file = TestFile::new("invalid");
        for contents in ["", "  ", "0", "-12", "hermit", "12abc"] {
            let mut file = test_file.open();
            file.set_len(0).unwrap();
            write!(file, "{contents}").unwrap();
            assert_eq!(
                recorded_process_id(&mut file).unwrap(),
                None,
                "{contents:?}"
            );
        }
    }

    #[test]
    fn accepts_surrounding_whitespace() {
        let test_file = TestFile::new("whitespace");
        let mut file = test_file.open();
        writeln!(file, " 4321").unwrap();
        assert_eq!(recorded_process_id(&mut file).unwrap(), Some(4321));
    }

    #[test]
    fn lock_is_exclusive_until_released() {
        let test_file = TestFile::new("exclusive");
        let holder = test_file.open();
        let other = test_file.open();

        assert!(try_lock(&holder).unwrap());
        assert!(!try_lock(&other).unwrap());

        drop(holder);
        assert!(try_lock(&other).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recognizes_its_own_executable() {
        let pid = i32::try_from(std::process::id()).unwrap();
        assert!(is_hermit(pid).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn executable_name_ignores_the_deleted_marker() {
        use std::path::Path;

        assert_eq!(
            executable_name(Path::new("/home/me/hermit/target/debug/hermit (deleted)")),
            Some("hermit")
        );
        assert_eq!(
            executable_name(Path::new("/usr/bin/hermit")),
            Some("hermit")
        );
        assert_ne!(executable_name(Path::new("/usr/bin/kate")), Some("hermit"));
    }
}
