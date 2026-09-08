//! Process construction shared by the core and the local server.
//!
//! RemoteDeck is a browser application, so ordinary helper processes must not
//! inherit a visible Windows console.  Keep that policy in one place instead
//! of relying on every call site to remember `CREATE_NO_WINDOW`.  Interactive
//! ConPTY children are intentionally separate: `portable-pty` owns their
//! `STARTUPINFOEX` and pseudo-console attachment, and those children are
//! created through [`portable_pty::CommandBuilder`] in `session.rs`.

use std::ffi::OsStr;

#[cfg(windows)]
use std::os::windows::process::CommandExt as _;

/// Creates a hidden ordinary child process.
///
/// On non-Windows platforms this is the same as `std::process::Command::new`.
/// On Windows the child receives `CREATE_NO_WINDOW`, which prevents a console
/// window from being created or inherited for command-line tools such as
/// OpenSSH, PowerShell, and `ssh-keygen`.
pub fn command<S: AsRef<OsStr>>(program: S) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    hide(&mut command);
    command
}

/// Creates a hidden Tokio child process.
///
/// This is the asynchronous counterpart to [`command`].
pub fn async_command<S: AsRef<OsStr>>(program: S) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    hide_async(&mut command);
    command
}

/// Applies the ordinary-process hidden-window policy to an existing command.
///
/// This is public so the server layer can configure a command that has to be
/// assembled before it is passed to a helper.  It must not be used for a
/// `portable_pty::CommandBuilder`; ConPTY has its own process creation path.
pub fn hide(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Applies the ordinary-process hidden-window policy to an existing Tokio
/// command.
pub fn hide_async(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(test)]
mod tests {
    use super::{async_command, command};
    use std::{
        fs,
        path::{Path, PathBuf},
        process::Stdio,
        time::Duration,
    };

    #[cfg(windows)]
    // Windows PowerShell and Add-Type can exceed five seconds on a cold hosted
    // runner. This deadline covers tool startup; the no-console assertion is
    // unchanged, and a timed-out async child is terminated on drop.
    const WINDOWS_CONSOLE_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

    #[cfg(windows)]
    const WINDOWS_CONSOLE_PROBE: &str = r#"
$signature = @'
using System;
using System.Runtime.InteropServices;
public static class RemoteDeckConsoleProbe {
    [DllImport("kernel32.dll")]
    public static extern IntPtr GetConsoleWindow();
}
'@;
Add-Type -TypeDefinition $signature;
[RemoteDeckConsoleProbe]::GetConsoleWindow().ToInt64()
"#;

    #[test]
    fn std_helper_constructs_a_runnable_hidden_process() {
        let mut child = if cfg!(windows) {
            let mut command = command("cmd.exe");
            command.args(["/C", "exit", "0"]);
            command
        } else {
            let mut command = command("sh");
            command.args(["-c", "exit 0"]);
            command
        }
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn helper child");

        let status = child.wait().expect("wait helper child");
        assert!(status.success());
    }

    #[tokio::test]
    async fn tokio_helper_constructs_a_runnable_hidden_process() {
        let mut child = if cfg!(windows) {
            let mut command = async_command("cmd.exe");
            command.args(["/C", "exit", "0"]);
            command
        } else {
            let mut command = async_command("sh");
            command.args(["-c", "exit 0"]);
            command
        }
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn async helper child");

        let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .expect("helper child timeout")
            .expect("wait helper child");
        assert!(status.success());
    }

    #[cfg(windows)]
    #[test]
    fn windows_helper_child_has_no_console_window() {
        let child = command("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                WINDOWS_CONSOLE_PROBE,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn PowerShell console probe");
        let output = child
            .wait_with_output()
            .expect("wait PowerShell console probe");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_async_helper_child_has_no_console_window() {
        let child = async_command("powershell.exe")
            .kill_on_drop(true)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                WINDOWS_CONSOLE_PROBE,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn async PowerShell console probe");
        let output = tokio::time::timeout(WINDOWS_CONSOLE_PROBE_TIMEOUT, child.wait_with_output())
            .await
            .expect("async PowerShell console probe timeout")
            .expect("wait async PowerShell console probe");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
    }

    #[test]
    fn ordinary_process_construction_stays_in_this_module() {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        collect_process_policy_violations(&source_root, &mut violations)
            .expect("scan core process construction");
        assert!(
            violations.is_empty(),
            "ordinary process construction must use process.rs: {violations:?}"
        );
    }

    fn collect_process_policy_violations(
        directory: &Path,
        violations: &mut Vec<PathBuf>,
    ) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                collect_process_policy_violations(&path, violations)?;
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
                || path.file_name().and_then(|name| name.to_str()) == Some("process.rs")
            {
                continue;
            }
            let source = fs::read_to_string(&path)?;
            if [
                "Command::new(",
                "std::process::Command::new(",
                "tokio::process::Command::new(",
                ".creation_flags(",
            ]
            .iter()
            .any(|needle| source.contains(needle))
            {
                violations.push(path);
            }
        }
        Ok(())
    }
}
