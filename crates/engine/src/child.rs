//! Linux's stand-in for the Windows job object Portal sits in: ffmpeg and the
//! cast helper must not outlive Portal, however it exits.
//!
//! `guard_children` starts a small guardian shell in a process group of its
//! own and keeps the write end of its input. Each contained child joins that
//! group. When Portal ends, even by `kill -9`, the kernel closes the pipe, the
//! guardian's `read` returns, and it kills the whole group. Kodi is not
//! contained: it runs in a session of its own and outlives Portal, as it does
//! on Windows. (`PR_SET_PDEATHSIG` is not used: it fires when the spawning
//! *thread* ends, and ffmpeg is often started from a short-lived blocking
//! thread.)

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The guardian's process group; unset until `guard_children` runs, as in tests.
static GROUP: OnceLock<i32> = OnceLock::new();

/// Starts the guardian. Called once, early in `main`.
pub fn guard_children() {
    if GROUP.get().is_some() {
        return;
    }
    let started = Command::new("/bin/sh")
        .args(["-c", "trap '' INT TERM HUP; read _; kill -KILL 0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    match started {
        Ok(mut guardian) => {
            let Ok(group) = i32::try_from(guardian.id()) else {
                return;
            };
            // Held open for the life of this process; the kernel closes it at exit.
            std::mem::forget(guardian.stdin.take());
            std::mem::forget(guardian);
            let _ = GROUP.set(group);
        }
        Err(err) => tracing::warn!("child guardian unavailable ({err}); child processes are not contained"),
    }
}

/// Puts a child in the guardian's group, and, when `low_priority`, a little
/// below normal priority (`nice` 5), as BELOW_NORMAL_PRIORITY_CLASS does on Windows.
pub fn contain(command: &mut Command, low_priority: bool) {
    if let Some(group) = GROUP.get() {
        command.process_group(*group);
    }
    if low_priority {
        // SAFETY: nice is async-signal-safe and touches no memory.
        unsafe {
            command.pre_exec(|| {
                libc::nice(5);
                Ok(())
            });
        }
    }
}

/// `contain` for a tokio command.
pub fn contain_tokio(command: &mut tokio::process::Command, low_priority: bool) {
    if let Some(group) = GROUP.get() {
        command.process_group(*group);
    }
    if low_priority {
        // SAFETY: as in `contain`.
        unsafe {
            command.pre_exec(|| {
                libc::nice(5);
                Ok(())
            });
        }
    }
}
