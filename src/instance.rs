use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const LOCK_NAME: &str = "copybot.lock";

pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

/// Exclusive lock so Telegram getUpdates is only polled by one process.
/// `check` must not call this.
pub fn acquire() -> Result<InstanceLock> {
    let path = Path::new(LOCK_NAME);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    if try_exclusive(&file).is_err() {
        let other = stale_or_live_pid(path)?.unwrap_or(0);
        bail!(
            "copybot already running{}.\n\
             One process only — Telegram 409 means a second instance is polling.\n\
             On the VPS: tmux kill-session -t copybot; pkill -f target/release/copybot; then ./run.sh",
            if other > 0 {
                format!(" (pid {other})")
            } else {
                String::new()
            }
        );
    }
    file.set_len(0)?;
    write!(file, "{}", std::process::id())?;
    file.flush()?;
    Ok(InstanceLock {
        _file: file,
        path: path.to_path_buf(),
    })
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn stale_or_live_pid(path: &Path) -> Result<Option<u32>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut s = String::new();
    File::open(path)?.read_to_string(&mut s)?;
    let Ok(pid) = s.trim().parse::<u32>() else {
        return Ok(None);
    };
    if pid == std::process::id() {
        return Ok(None);
    }
    if pid_alive(pid) {
        return Ok(Some(pid));
    }
    Ok(None)
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
fn try_exclusive(file: &File) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    let rc = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    bail!(
        "copybot already running (lock busy).\n\
         tmux kill-session -t copybot; pkill -f target/release/copybot; then ./run.sh"
    );
}

#[cfg(not(unix))]
fn try_exclusive(_file: &File) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pid_alive;

    #[test]
    fn self_pid_is_alive_on_linux() {
        #[cfg(target_os = "linux")]
        assert!(pid_alive(std::process::id()));
        #[cfg(not(target_os = "linux"))]
        let _ = pid_alive(1);
    }
}
