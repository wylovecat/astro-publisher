//! Refuse to run a second copy of the app (plan §4.2).
//!
//! Two instances are not merely untidy: each would spawn its own `astro dev`
//! server on the same port, each could run a publish pipeline, and each would
//! write the same config file. The fix has to survive a crash, so the lock is a
//! file holding the owner's pid, and a lock whose owner is gone is treated as
//! stale and taken over.
//!
//! A named mutex would be tidier on Windows, but this has to work on Linux Mint
//! too, and it is the same amount of code everywhere. The pid check is the one
//! part that cannot be delegated: a stale lock must never lock the user out of
//! their own tool.
//!
//! Known limitation: pid reuse. If the app is killed and the operating system
//! hands its pid to an unrelated process before the next launch, the next
//! launch reads a "live" pid and refuses to start. Rare enough not to warrant a
//! heartbeat file; the cure is deleting `instance.lock` by hand.

use std::path::{Path, PathBuf};

/// What happened when the lock was claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The lock is now ours.
    Acquired {
        /// The pid of the instance we displaced, if any. Worth logging: a stale
        /// lock here means a previous run was killed rather than closed.
        displaced: Option<u32>,
    },
    /// Another live instance holds it, and we are the one that has to stop.
    InUse { pid: u32 },
}

/// Held for as long as one instance runs. Dropping it removes the lock file, so
/// a clean exit leaves nothing behind.
pub struct InstanceLock {
    path: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Try to become the one instance.
pub fn claim() -> Result<(Claim, Option<InstanceLock>), String> {
    let Some(path) = lock_path() else {
        // No config directory available: run anyway rather than refuse to start.
        return Ok((Claim::Acquired { displaced: None }, None));
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("无法创建配置目录 {}：{err}", parent.display()))?;
    }

    if create_exclusive(&path).is_ok() {
        return finish(&path, None);
    }

    // The file exists. Its owner being alive is the only thing that makes this
    // a real conflict.
    match read_pid(&path) {
        Some(pid) if pid != std::process::id() && is_running(pid) => {
            Ok((Claim::InUse { pid }, None))
        }
        stale => {
            std::fs::remove_file(&path)
                .map_err(|err| format!("无法清理失效的锁 {}：{err}", path.display()))?;
            create_exclusive(&path).map_err(|()| {
                format!("无法获取实例锁 {}（文件仍被占用）", path.display())
            })?;
            finish(&path, stale)
        }
    }
}

/// Stamp our own pid in and wrap the path into the guard.
fn finish(path: &Path, displaced: Option<u32>) -> Result<(Claim, Option<InstanceLock>), String> {
    write_pid(path, std::process::id()).map_err(|err| {
        format!("无法写入实例锁 {}：{err}", path.display())
    })?;
    Ok((
        Claim::Acquired { displaced },
        Some(InstanceLock {
            path: path.to_path_buf(),
        }),
    ))
}

fn lock_path() -> Option<PathBuf> {
    astro_publisher_core::config::Config::dir().map(|dir| dir.join("instance.lock"))
}

/// Create the file only if it does not exist yet.
fn create_exclusive(path: &Path) -> Result<(), ()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map(|_| ())
        .map_err(|_| ())
}

fn write_pid(path: &Path, pid: u32) -> std::io::Result<()> {
    std::fs::write(path, format!("{pid}\n"))
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse::<u32>().ok()
}

/// Is a process with this pid alive right now?
pub fn is_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> *mut c_void;
            fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }
        // `SYNCHRONIZE` is not optional here: `PROCESS_QUERY_LIMITED_INFORMATION`
        // alone produces a handle that `WaitForSingleObject` refuses with
        // WAIT_FAILED, because waiting *is* the operation that needs the right.
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        const SYNCHRONIZE: u32 = 0x0010_0000;
        const WAIT_TIMEOUT: u32 = 0x0000_0102;

        // SAFETY: a null handle is the documented way to ask "does this process
        // exist"; a real handle is closed again before returning either way.
        unsafe {
            let handle = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            // Zero timeout: this asks whether the process has already signalled
            // rather than waiting for it to.
            let signalled = WaitForSingleObject(handle, 0);
            CloseHandle(handle);
            signalled == WAIT_TIMEOUT
        }
    }
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

/// Tell the user, in the only channel a GUI binary has.
///
/// The release build is a windows-subsystem binary: no console, so `eprintln!`
/// goes nowhere. A second copy that silently refuses to open looks like a
/// broken app, so Windows gets a message box. Elsewhere the text goes to
/// stderr, where anyone running from a terminal will see it.
pub fn notify_already_running(pid: u32) {
    let message = format!(
        "astro-publisher 已经在运行了（进程 {pid}）。\n\n\
         请切到已打开的窗口；如果找不到，可以在任务管理器里结束进程 {pid}。"
    );

    #[cfg(windows)]
    {
        use std::ffi::c_void;
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut c_void,
                text: *const u16,
                caption: *const u16,
                kind: u32,
            ) -> i32;
        }
        const MB_OK: u32 = 0x0000_0000;
        const MB_ICONINFORMATION: u32 = 0x0000_0040;
        // SAFETY: both wide strings are NUL-terminated and outlive the call.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                wide(&message).as_ptr(),
                wide("astro-publisher").as_ptr(),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }

    #[cfg(not(windows))]
    {
        eprintln!("{message}");
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_counts_as_running() {
        assert!(is_running(std::process::id()));
    }

    #[test]
    fn an_unused_pid_does_not() {
        // Pid 0 is never a user process, and u32::MAX is not a pid any system
        // hands out.
        assert!(!is_running(0));
        assert!(!is_running(u32::MAX));
    }

    #[test]
    fn a_lock_file_is_claimed_by_exactly_one_writer() {
        let dir = std::env::temp_dir().join(format!("astro-publisher-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("instance.lock");

        assert!(create_exclusive(&path).is_ok(), "the first claim wins");
        assert!(
            create_exclusive(&path).is_err(),
            "a second exclusive create must fail"
        );

        write_pid(&path, 0).expect("stamp a pid");
        assert_eq!(read_pid(&path), Some(0));
        assert!(!is_running(0), "pid 0 is stale, so the lock is takeable");

        std::fs::remove_file(&path).expect("clean up");
        std::fs::remove_dir(&dir).ok();
    }
}
