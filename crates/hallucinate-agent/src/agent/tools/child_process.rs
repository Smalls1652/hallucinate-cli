//! Cancellation-safe cleanup for shell commands and their descendants.

#[cfg(unix)]
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use tokio::process::{Child, Command};

/// Start a shell in its own process group so dropping an in-flight tool can
/// terminate the shell and any commands it spawned, not just the direct child.
pub(super) fn spawn_shell(command: &mut Command) -> Result<ShellChild, std::io::Error> {
    #[cfg(unix)]
    command.process_group(0);

    command.kill_on_drop(true);

    let child = command.spawn()?;

    Ok(ShellChild {
        #[cfg(unix)]
        group_id: child.id().expect("spawned process has an ID"),
        child,
        running: true,
    })
}

/// Wrapper around a Tokio `Child` that tracks process group membership and
/// lifecycle state for cancellation-safe cleanup of shell commands.
pub(super) struct ShellChild {
    /// The underlying Tokio child process handle.
    pub(super) child: Child,

    /// The Unix process group ID for sending signals to descendants.
    #[cfg(unix)]
    group_id: u32,

    /// Whether the child is still running and needs termination on drop.
    running: bool,
}

impl ShellChild {
    /// Kill the entire group before reaping the direct child. This is also
    /// used on timeout; `Drop` covers cancellation while awaiting the child.
    pub(super) fn terminate(&mut self) -> Result<(), std::io::Error> {
        #[cfg(unix)]
        {
            let group = Pid::from_raw(-(self.group_id as i32));

            match kill(group, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {},
                Err(error) => return Err(std::io::Error::from_raw_os_error(error as i32)),
            }
        }

        #[cfg(not(unix))]
        self.child.start_kill()?;

        Ok(())
    }

    /// Mark the child as no longer running so it won't be terminated on drop.
    pub(super) fn finished(&mut self) {
        self.running = false;
    }
}

impl Drop for ShellChild {
    fn drop(&mut self) {
        if self.running {
            let _ = self.terminate();
        }
    }
}
