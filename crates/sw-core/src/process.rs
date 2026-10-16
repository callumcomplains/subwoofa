//! Starting helper programs (FFmpeg, ffprobe, the DTS verifier) without a console window.
//!
//! The app is a Windows GUI program, and Windows gives every console program started by a GUI program its own
//! console window, which flashes on screen while the program runs. `CREATE_NO_WINDOW` (from the Win32 process
//! creation flags) runs it with no window; standard input and output pipes work as usual.

use std::ffi::OsStr;
use std::process::Command;

/// Win32 `CREATE_NO_WINDOW`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `Command` for `program` that opens no console window.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_still_arrives() {
        let program = if cfg!(windows) { "cmd" } else { "sh" };
        let args: &[&str] = if cfg!(windows) {
            &["/C", "echo hi"]
        } else {
            &["-c", "echo hi"]
        };
        let out = command(program).args(args).output().expect("the shell starts");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
    }
}
