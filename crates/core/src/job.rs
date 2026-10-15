//! Kill child processes when the app dies — including when it is killed.
//!
//! The `astro dev` preview server and the publish pipeline's `pnpm` / `git` calls
//! are child processes of this one. Cleaning them up on a normal exit is easy
//! (`taskkill /T` from a window-close hook). The case that leaks is the one that
//! actually happens: the user ends the app from the task manager, or Windows
//! kills it, and the `node` server keeps holding port 4321 with nothing left to
//! stop it.
//!
//! A Windows job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` closes that
//! hole — the kernel tears the whole job down when the last handle to it goes
//! away, and a process handle dies with the process that opened it, however that
//! process died. On other platforms this is a no-op: nothing here spawns
//! long-lived children that outlive the app by design, and the existing
//! window-close hook still runs.
//!
//! Declared through raw `kernel32` FFI rather than a crate: it is four functions
//! and two constants, and the tool otherwise has no Windows dependencies.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    /// A `kernel32` `HANDLE`. Kept raw here and wrapped in [`KillOnCloseJob`],
    /// which is the local type that carries the `Send`/`Sync` impls — raw
    /// pointers are foreign types and cannot have traits implemented for them.
    pub type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    /// `JobObjectExtendedLimitInformation`
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    /// "Terminate all processes associated with the job when the last handle to
    /// the job is closed."
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SET_QUOTA: u32 = 0x0100;

    unsafe extern "system" {
        pub fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        pub fn SetInformationJobObject(
            job: Handle,
            class: i32,
            information: *const c_void,
            length: u32,
        ) -> i32;
        pub fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        pub fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        pub fn CloseHandle(handle: Handle) -> i32;
    }

    /// Create a job that kills its processes when the last handle closes.
    pub fn create_kill_on_close() -> Handle {
        // SAFETY: a null attributes pointer means default security, and a null
        // name means an unnamed job — both are the documented defaults.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return std::ptr::null_mut();
        }

        let mut limits = ExtendedLimitInformation::default();
        limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the exact struct the class number asks for, and
        // the length is its size.
        let ok = unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                std::ptr::from_ref(&limits).cast(),
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            )
        };
        if ok == 0 {
            // SAFETY: `job` is a real handle that nothing else owns.
            unsafe { CloseHandle(job) };
            return std::ptr::null_mut();
        }
        job
    }

    /// Put an already-running process into the job.
    pub fn assign(job: Handle, pid: u32) -> bool {
        // SAFETY: a non-null job handle we own; the access mask is the minimum
        // needed to place a process in a job.
        let process = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SET_QUOTA, 0, pid) };
        if process.is_null() {
            return false;
        }
        // SAFETY: both handles are open; `pid` was just opened by us.
        let ok = unsafe { AssignProcessToJobObject(job, process) };
        // SAFETY: closing the handle we opened above.
        unsafe { CloseHandle(process) };
        ok != 0
    }
}

#[cfg(windows)]
use imp::Handle;

/// A job object whose processes die with it (plan §4.2).
///
/// Held for the whole run of the app and deliberately never closed on purpose:
/// closing it is exactly the "kill the children" signal.
#[cfg(windows)]
pub struct KillOnCloseJob(Handle);

#[cfg(windows)]
// SAFETY: see the `Handle` impls — the value is a process-global kernel handle.
unsafe impl Send for KillOnCloseJob {}
#[cfg(windows)]
unsafe impl Sync for KillOnCloseJob {}

#[cfg(windows)]
impl KillOnCloseJob {
    /// Create the job, or `None` if the OS refused (an over-restrictive
    /// environment should degrade to "children may outlive us", not fail).
    pub fn new() -> Option<Self> {
        let handle = imp::create_kill_on_close();
        if handle.is_null() {
            None
        } else {
            Some(Self(handle))
        }
    }

    /// Move a running child into the job.
    ///
    /// Returns whether the OS accepted it. It legitimately refuses when the
    /// child is already inside a job that forbids nesting — for example when
    /// this app is itself run from a job-constrained host.
    pub fn adopt(&self, pid: u32) -> bool {
        imp::assign(self.0, pid)
    }
}

#[cfg(windows)]
impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: `self.0` is the handle this value owns.
        unsafe { imp::CloseHandle(self.0) };
    }
}

#[cfg(not(windows))]
pub struct KillOnCloseJob;

#[cfg(not(windows))]
impl KillOnCloseJob {
    /// Nothing to do off Windows.
    pub fn new() -> Option<Self> {
        None
    }

    /// No-op off Windows.
    pub fn adopt(&self, _pid: u32) -> bool {
        false
    }
}

use std::sync::OnceLock;

static JOB: OnceLock<Option<KillOnCloseJob>> = OnceLock::new();

/// Create the app-wide job. Call once, early, in the GUI entry point.
///
/// The CLI and the MCP server do not call it: they have no window to close and
/// no long-lived children, so a job object would only add a failure mode.
pub fn install() {
    let _ = JOB.set(KillOnCloseJob::new());
}

/// Put a just-spawned child into the app-wide job, if there is one.
///
/// Safe to call unconditionally from every spawn site: it does nothing unless
/// [`install`] ran first, and never fails the spawn.
pub fn adopt(pid: u32) {
    if let Some(Some(job)) = JOB.get() {
        job.adopt(pid);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn closing_the_job_kills_a_process_assigned_to_it() {
        let Some(job) = KillOnCloseJob::new() else {
            panic!("the OS refused to create a job object");
        };

        let mut child = Command::new("cmd")
            .args(["/c", "ping", "-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a long-lived child");

        if !job.adopt(child.id()) {
            // Assignment is denied when the process is already inside a job
            // that disallows nesting (a job-constrained CI host, for example).
            // That is an environment fact, not a defect in the code path.
            eprintln!("skipping: this test process is already inside a job object");
            let _ = child.kill();
            return;
        }

        assert!(
            child.try_wait().expect("query the child").is_none(),
            "the child should still be running right after assignment"
        );

        // Dropping the handle is the same event as the app process dying.
        drop(job);

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if child.try_wait().expect("query the child").is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the child outlived the job object that was supposed to kill it"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
