//! Spawning helpers shared by every crate that starts a process from the
//! daemon. On Windows a bare `Command` opens a console window for the
//! child — a version probe or a hook flashes a black box on the operator's
//! screen every time it runs. `quiet_command` sets `CREATE_NO_WINDOW`;
//! use it for anything the daemon runs, not just git.

/// A `std::process::Command` that never shows a window.
pub fn quiet_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}
