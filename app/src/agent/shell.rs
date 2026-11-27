//! The shell the agent's `bash` tool runs commands in, on each platform, and
//! the process tree a command starts, so that a stop or a timeout ends all
//! of it and not only the shell.
//!
//! - Unix: `/bin/sh -c`. The command gets its own process group, and
//!   stopping it kills the group.
//! - Windows: Git for Windows' bash when it is installed, so commands keep
//!   POSIX semantics; otherwise PowerShell. The command runs in a Job
//!   Object, and stopping it terminates the job.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The shell commands run in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shell {
    /// `/bin/sh`.
    Posix,
    /// Git for Windows' `bash.exe`.
    #[cfg_attr(not(windows), allow(dead_code))]
    GitBash(PathBuf),
    /// `pwsh.exe` (PowerShell 7) or `powershell.exe` (Windows PowerShell).
    #[cfg_attr(not(windows), allow(dead_code))]
    PowerShell(PathBuf),
}

/// This machine's shell, found once.
pub fn current() -> &'static Shell {
    static FOUND: OnceLock<Shell> = OnceLock::new();
    FOUND.get_or_init(probe)
}

#[cfg(not(windows))]
fn probe() -> Shell {
    Shell::Posix
}

#[cfg(windows)]
fn probe() -> Shell {
    let under = |var: &str, parts: &[&str]| {
        crate::envvar::get_os(var).map(|base| {
            parts
                .iter()
                .fold(PathBuf::from(base), |p, part| p.join(part))
        })
    };
    // Git for Windows, machine-wide or per user. `bin\bash.exe` sets up the
    // Git environment (PATH with the Unix tools) before it starts bash.
    let installed = [
        under("ProgramFiles", &["Git", "bin", "bash.exe"]),
        under("ProgramW6432", &["Git", "bin", "bash.exe"]),
        under("LOCALAPPDATA", &["Programs", "Git", "bin", "bash.exe"]),
    ];
    if let Some(bash) = installed.into_iter().flatten().find(|p| p.is_file()) {
        return Shell::GitBash(bash);
    }
    // A bash on PATH, but not WSL's launcher (System32\bash.exe, or the
    // Store's alias under WindowsApps): that runs Linux, not this machine.
    let wsl = |p: &PathBuf| {
        let s = p.to_string_lossy().to_lowercase();
        s.contains("\\system32\\") || s.contains("\\windowsapps\\")
    };
    if let Some(bash) = on_path("bash.exe").into_iter().find(|p| !wsl(p)) {
        return Shell::GitBash(bash);
    }
    let powershell = on_path("pwsh.exe")
        .into_iter()
        .next()
        .or_else(|| on_path("powershell.exe").into_iter().next())
        .or_else(|| {
            under(
                "SystemRoot",
                &["System32", "WindowsPowerShell", "v1.0", "powershell.exe"],
            )
            .filter(|p| p.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("powershell.exe"));
    Shell::PowerShell(powershell)
}

/// Every file called `name` on PATH, in order.
#[cfg(windows)]
fn on_path(name: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(name))
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default()
}

impl Shell {
    /// How the shell is named to the model.
    pub fn name(&self) -> &'static str {
        match self {
            Shell::Posix => "sh",
            Shell::GitBash(_) => "Git Bash",
            Shell::PowerShell(_) => "PowerShell",
        }
    }

    /// The program and arguments that run `command` (a command line in
    /// this shell's language).
    pub fn argv(&self, command: &str) -> Vec<String> {
        match self {
            Shell::Posix => vec!["/bin/sh".into(), "-c".into(), command.into()],
            Shell::GitBash(bash) => vec![
                bash.to_string_lossy().into_owned(),
                "-c".into(),
                command.into(),
            ],
            Shell::PowerShell(ps) => {
                use base64::Engine as _;
                // UTF-8 output, so the pipe doesn't carry the console's code
                // page. Encoded (UTF-16LE, base64) so quotes survive the
                // Windows command line, which PowerShell parses its own way.
                let script = format!(
                    "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n$OutputEncoding = [System.Text.Encoding]::UTF8\n{command}"
                );
                let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                vec![
                    ps.to_string_lossy().into_owned(),
                    "-NoLogo".into(),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-EncodedCommand".into(),
                    base64::engine::general_purpose::STANDARD.encode(utf16),
                ]
            }
        }
    }
}

/// Sets `cmd` up so everything it starts can be stopped together.
pub fn prepare(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Everything a started command started. Dropped while still armed (the
/// run was abandoned), it kills the lot; `release` leaves a command that
/// finished, and whatever it left running on purpose, alone.
pub struct Tree {
    #[cfg(unix)]
    group: Option<i32>,
    #[cfg(windows)]
    job: Option<Job>,
    armed: bool,
}

impl Tree {
    /// The tree of `child`, started with `prepare`.
    pub fn of(child: &tokio::process::Child) -> Tree {
        #[cfg(not(any(unix, windows)))]
        let _ = child;
        Tree {
            // `process_group(0)` makes the child its group's leader.
            #[cfg(unix)]
            group: child.id().and_then(|id| i32::try_from(id).ok()),
            #[cfg(windows)]
            job: Job::holding(child),
            armed: true,
        }
    }

    /// Kills every process in the tree. The caller still kills and reaps
    /// the child itself.
    pub fn kill(&mut self) {
        self.armed = false;
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // SAFETY: plain syscall; the leader isn't reaped yet, so the
            // group id can't have been reused.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            job.terminate();
        }
    }

    /// The command finished: leave the rest alone.
    pub fn release(mut self) {
        self.armed = false;
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if self.armed {
            self.kill();
        }
    }
}

/// A Job Object holding the command and everything it starts.
#[cfg(windows)]
struct Job(usize);

#[cfg(windows)]
impl Job {
    fn holding(child: &tokio::process::Child) -> Option<Job> {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
        let process = child.raw_handle()? as HANDLE;
        // SAFETY: an anonymous job with default security; `process` is the
        // child's handle, open while `child` lives. Processes the child
        // starts from now on join the job too.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            if AssignProcessToJobObject(job, process) == 0 {
                CloseHandle(job);
                return None;
            }
            Some(Job(job as usize))
        }
    }

    fn terminate(self) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: the handle is ours until `Drop` closes it.
        unsafe {
            TerminateJobObject(self.0 as windows_sys::Win32::Foundation::HANDLE, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        // Closing the handle doesn't end the processes (no
        // KILL_ON_JOB_CLOSE): only `terminate` does.
        // SAFETY: closes the handle `holding` opened, once.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(
                self.0 as windows_sys::Win32::Foundation::HANDLE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn unix_runs_sh() {
        assert_eq!(current(), &Shell::Posix);
        assert_eq!(current().argv("true"), ["/bin/sh", "-c", "true"]);
    }

    #[test]
    fn powershell_gets_the_command_encoded() {
        use base64::Engine as _;
        let argv = Shell::PowerShell(PathBuf::from("pwsh.exe")).argv("echo \"hi\"");
        assert_eq!(
            argv[..5],
            [
                "pwsh.exe",
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand"
            ]
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&argv[5])
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert!(
            String::from_utf16(&units)
                .unwrap()
                .ends_with("\necho \"hi\"")
        );
    }
}
