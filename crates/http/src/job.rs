//! Puts Portal in a Windows job object that kills its members when Portal
//! exits, however it exits. ffmpeg and the cast helper are children, so a
//! crash or a Task Manager kill cannot leave them transcoding in the
//! background. Kodi is started with `CREATE_BREAKAWAY_FROM_JOB`, so a Kodi
//! Portal started outlives it and is attached again on the next launch.

#[cfg(windows)]
pub fn contain_children() {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: plain Win32 calls on a handle this function owns. The handle is
    // leaked on purpose: the job must live exactly as long as this process.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            tracing::warn!("job object unavailable; child processes are not contained");
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set == 0 || AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            tracing::warn!("job object refused; child processes are not contained");
        }
    }
}

/// Linux: a guardian process kills every contained child when Portal ends;
/// see `portal_engine::child`.
#[cfg(not(windows))]
pub fn contain_children() {
    portal_engine::child::guard_children();
}
