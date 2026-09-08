//! Narrow native helpers used by the local service lifecycle.
//!
//! The web application never supplies a URL to this module.  The service uses
//! it only to open its own loopback origin after a successful bind.

use remotedeck_core::error::{AppError, AppResult};
use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeError {
    InvalidLoopbackUrl(&'static str),
    BrowserLaunchFailed,
    #[cfg(not(target_os = "windows"))]
    UnsupportedPlatform,
}

impl fmt::Display for NativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLoopbackUrl(reason) => {
                write!(formatter, "invalid local browser URL: {reason}")
            }
            Self::BrowserLaunchFailed => {
                formatter.write_str("Windows could not open the default browser")
            }
            #[cfg(not(target_os = "windows"))]
            Self::UnsupportedPlatform => {
                formatter.write_str("opening the system browser is supported only on Windows")
            }
        }
    }
}

impl Error for NativeError {}

/// Validates the exact origin shape emitted by the local HTTP listener.
///
/// This deliberately does not accept host names, user-info, alternate schemes,
/// query strings, or arbitrary paths. The only suffix accepted is a one-time
/// 64-hex-character browser ticket in a fragment, which never reaches HTTP.
/// Keeping the accepted grammar narrow prevents a lifecycle or compatibility
/// callback from becoming an arbitrary URL opener.
pub(crate) fn validate_loopback_url(value: &str) -> Result<(), NativeError> {
    let Some(remainder) = value.strip_prefix("http://") else {
        return Err(NativeError::InvalidLoopbackUrl("only http is permitted"));
    };
    let (authority, suffix) = remainder.split_once('/').unwrap_or((remainder, ""));
    if !suffix.is_empty()
        && !(suffix
            .strip_prefix("#ticket=")
            .is_some_and(is_browser_ticket))
    {
        return Err(NativeError::InvalidLoopbackUrl(
            "only a one-time browser ticket fragment may follow the origin",
        ));
    }

    let port = if let Some(value) = authority.strip_prefix("127.0.0.1:") {
        value
    } else if let Some(value) = authority.strip_prefix("[::1]:") {
        value
    } else {
        return Err(NativeError::InvalidLoopbackUrl(
            "the host must be a numeric loopback address",
        ));
    };

    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(NativeError::InvalidLoopbackUrl("the port is malformed"));
    }
    if port.parse::<u16>().ok().filter(|port| *port != 0).is_none() {
        return Err(NativeError::InvalidLoopbackUrl(
            "the port is outside 1..=65535",
        ));
    }
    Ok(())
}

fn is_browser_ticket(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Opens a service-owned loopback URL with the user's default browser.
pub(crate) fn open_url(value: &str) -> Result<(), NativeError> {
    validate_loopback_url(value)?;

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::HWND,
            UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        };

        let operation = wide_null("open");
        let url = wide_null(value);
        // ShellExecuteW is invoked directly with the validated URL, avoiding a
        // command interpreter and its quoting/injection surface.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut::<std::ffi::c_void>() as HWND,
                operation.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if (result as isize) <= 32 {
            return Err(NativeError::BrowserLaunchFailed);
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = value;
        Err(NativeError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub(crate) async fn pick_local_path(directory: bool) -> Option<String> {
    let dialog = rfd::AsyncFileDialog::new().set_title(if directory {
        "Select a local folder"
    } else {
        "Select a local file"
    });
    let selection = if directory {
        dialog.pick_folder().await
    } else {
        dialog.pick_file().await
    };
    selection.map(|handle| handle.path().to_string_lossy().into_owned())
}

pub(crate) async fn pick_save_path(suggested_name: String) -> AppResult<Option<String>> {
    validate_suggested_filename(&suggested_name)?;
    Ok(rfd::AsyncFileDialog::new()
        .set_title("Select a local destination")
        .set_file_name(suggested_name.trim())
        .save_file()
        .await
        .map(|handle| handle.path().to_string_lossy().into_owned()))
}

pub(crate) fn validate_suggested_filename(value: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 255
        || matches!(value, "." | "..")
        || value.contains(['/', '\\', ':'])
        || value.chars().any(char::is_control)
        || Path::new(value).file_name().and_then(|name| name.to_str()) != Some(value)
    {
        return Err(AppError::Validation(
            "suggested file name must be one local file name".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) async fn pick_diagnostics_path(suggested_name: String) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Export RemoteDeck diagnostics")
        .set_file_name(suggested_name)
        .add_filter("ZIP archive", &["zip"])
        .save_file()
        .await
        .map(|file| file.path().to_path_buf())
}

pub(crate) async fn confirm_process_kill(alias: &str, pid: u32, user: &str, command: &str) -> bool {
    let preview: String = command.chars().take(240).collect();
    rfd::AsyncMessageDialog::new().set_level(rfd::MessageLevel::Warning)
        .set_title("RemoteDeck: confirm SIGKILL")
        .set_description(format!("Host: {}\nPID: {pid}\nUser: {user}\nCommand: {preview}\n\nSIGKILL cannot be handled or deferred by the process. Continue?", alias.replace(['\r', '\n'], " ")))
        .set_buttons(rfd::MessageButtons::YesNo).show().await == rfd::MessageDialogResult::Yes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_numeric_loopback_origins() {
        assert!(validate_loopback_url("http://127.0.0.1:49152").is_ok());
        assert!(validate_loopback_url("http://[::1]:49152").is_ok());
        assert!(validate_loopback_url("http://127.0.0.1:49152/").is_ok());
        assert!(
            validate_loopback_url(&format!(
                "http://127.0.0.1:49152/#ticket={}",
                "a".repeat(64)
            ))
            .is_ok()
        );
        assert!(validate_loopback_url("https://127.0.0.1:49152").is_err());
        assert!(validate_loopback_url("http://localhost:49152").is_err());
        assert!(validate_loopback_url("http://127.0.0.1:0").is_err());
        assert!(
            validate_loopback_url("http://127.0.0.1:49152/?next=https://example.test").is_err()
        );
        assert!(validate_loopback_url("http://127.0.0.1:49152/path").is_err());
        assert!(validate_loopback_url("http://127.0.0.1:49152/#ticket=short").is_err());
        assert!(validate_loopback_url("http://127.0.0.1:49152/#ticket=zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
        assert!(validate_loopback_url("http://127.0.0.1:49152#fragment").is_err());
    }
}
