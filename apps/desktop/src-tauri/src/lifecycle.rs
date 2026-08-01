//! Native application lifecycle helpers.
//!
//! This module owns behavior that must not be delegated to the webview: the
//! system tray, close-to-tray handling, explicit full exit, hidden startup and
//! the Windows logon registration.

use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt, io,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use tauri::{
    AppHandle, CloseRequestApi, Manager, Runtime,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

pub const MAIN_WINDOW_LABEL: &str = "main";
pub const HIDDEN_LAUNCH_ARGUMENT: &str = "--hidden";

const TRAY_ICON_ID: &str = "remotedeck.tray";
const TRAY_SHOW_MENU_ID: &str = "remotedeck.tray.show";
const TRAY_EXIT_MENU_ID: &str = "remotedeck.tray.exit";
const WINDOWS_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const WINDOWS_RUN_VALUE_NAME: &str = "RemoteDeck";
const MAX_REGISTRY_DIAGNOSTIC_BYTES: usize = 4 * 1024;

/// State shared by close events and explicit exit actions.
///
/// A tray exit marks the state before asking Tauri to exit. This prevents a
/// close-to-tray handler from turning that explicit exit into another hide.
#[derive(Debug, Default)]
pub struct LifecycleState {
    full_exit_requested: AtomicBool,
}

impl LifecycleState {
    pub fn request_full_exit(&self) {
        self.full_exit_requested.store(true, Ordering::Release);
    }

    pub fn is_full_exit_requested(&self) -> bool {
        self.full_exit_requested.load(Ordering::Acquire)
    }
}

/// Returns `true` only when an exact `--hidden` argument is present.
///
/// The comparison is performed on `OsStr` values, so no lossy conversion or
/// substring matching can accidentally enable hidden mode.
pub fn is_hidden_launch<I, S>(arguments: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    arguments
        .into_iter()
        .any(|argument| argument.as_ref() == OsStr::new(HIDDEN_LAUNCH_ARGUMENT))
}

pub fn current_launch_is_hidden() -> bool {
    is_hidden_launch(std::env::args_os())
}

/// Pure close policy used by both the native event handler and unit tests.
pub fn should_hide_on_close(close_to_tray: bool, full_exit_requested: bool) -> bool {
    close_to_tray && !full_exit_requested
}

/// Shows, restores and focuses the main window.
///
/// `Ok(false)` means the configured main window does not currently exist.
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<bool> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Ok(false);
    };

    if window.is_minimized()? {
        window.unminimize()?;
    }
    window.show()?;
    window.set_focus()?;
    Ok(true)
}

/// Hides the main window without destroying its webview state.
///
/// `Ok(false)` means the configured main window does not currently exist.
pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<bool> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        return Ok(false);
    };

    window.hide()?;
    Ok(true)
}

/// Applies close-to-tray behavior to a main-window close request.
///
/// The close is prevented only after the window was hidden successfully. If
/// the window is missing or hiding fails, the native close request is allowed
/// to continue instead of leaving an invisible, unreachable process behind.
pub fn handle_close_requested<R: Runtime>(
    app: &AppHandle<R>,
    api: &CloseRequestApi,
    close_to_tray: bool,
    lifecycle: &LifecycleState,
) -> tauri::Result<bool> {
    if !should_hide_on_close(close_to_tray, lifecycle.is_full_exit_requested()) {
        return Ok(false);
    }

    if hide_main_window(app)? {
        api.prevent_close();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Marks this as an intentional full exit and terminates the Tauri event loop.
pub fn request_full_exit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(lifecycle) = app.try_state::<LifecycleState>() {
        lifecycle.request_full_exit();
    }
    app.exit(0);
}

/// Installs the RemoteDeck system tray.
///
/// The returned handle is also registered in Tauri's resource table, but the
/// caller may retain it when later tray updates are required.
pub fn install_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let show_item = MenuItem::with_id(
        app,
        TRAY_SHOW_MENU_ID,
        "Show RemoteDeck",
        true,
        None::<&str>,
    )?;
    let exit_item = MenuItem::with_id(
        app,
        TRAY_EXIT_MENU_ID,
        "Exit RemoteDeck",
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&show_item, &exit_item])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ICON_ID)
        .menu(&menu)
        .tooltip("RemoteDeck")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == TRAY_SHOW_MENU_ID {
                let _ = show_main_window(app);
            } else if event.id() == TRAY_EXIT_MENU_ID {
                request_full_exit(app);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = show_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)
}

#[derive(Debug)]
pub enum LaunchAtLoginError {
    #[cfg(not(target_os = "windows"))]
    UnsupportedPlatform,
    CurrentExecutable(io::Error),
    InvalidExecutablePath(&'static str),
    RegistryUnavailable(io::Error),
    RegistryCommandFailed {
        operation: &'static str,
        exit_code: Option<i32>,
        diagnostic: String,
    },
}

impl fmt::Display for LaunchAtLoginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(not(target_os = "windows"))]
            Self::UnsupportedPlatform => {
                formatter.write_str("launch at login is supported only on Windows")
            }
            Self::CurrentExecutable(error) => {
                write!(
                    formatter,
                    "failed to locate the RemoteDeck executable: {error}"
                )
            }
            Self::InvalidExecutablePath(reason) => {
                write!(
                    formatter,
                    "cannot register the RemoteDeck executable: {reason}"
                )
            }
            Self::RegistryUnavailable(error) => {
                write!(
                    formatter,
                    "failed to start the Windows registry tool: {error}"
                )
            }
            Self::RegistryCommandFailed {
                operation,
                exit_code,
                diagnostic,
            } => {
                write!(
                    formatter,
                    "failed to {operation} launch at login (exit code {exit_code:?}): {diagnostic}"
                )
            }
        }
    }
}

impl Error for LaunchAtLoginError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CurrentExecutable(error) | Self::RegistryUnavailable(error) => Some(error),
            _ => None,
        }
    }
}

/// Enables or disables the per-user Windows logon entry.
///
/// `reg.exe` is launched directly with a fixed key, value name and argument
/// vector. No shell is involved, so metacharacters in an installation path are
/// never evaluated as commands.
pub fn set_launch_at_login(enabled: bool) -> Result<(), LaunchAtLoginError> {
    #[cfg(target_os = "windows")]
    {
        let (operation, arguments) = if enabled {
            let executable =
                std::env::current_exe().map_err(LaunchAtLoginError::CurrentExecutable)?;
            let launch_value = build_startup_value(&executable)?;
            ("enable", registry_add_arguments(&launch_value))
        } else {
            ("disable", registry_delete_arguments())
        };

        run_registry_command(operation, &arguments)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        Err(LaunchAtLoginError::UnsupportedPlatform)
    }
}

fn build_startup_value(executable: &Path) -> Result<OsString, LaunchAtLoginError> {
    if !executable.is_absolute() {
        return Err(LaunchAtLoginError::InvalidExecutablePath(
            "the path is not absolute",
        ));
    }
    if executable.file_name().is_none() {
        return Err(LaunchAtLoginError::InvalidExecutablePath(
            "the path does not identify a file",
        ));
    }

    let executable_text = executable
        .to_str()
        .ok_or(LaunchAtLoginError::InvalidExecutablePath(
            "the path is not valid Unicode",
        ))?;
    if executable_text
        .chars()
        .any(|character| character == '"' || character.is_control())
    {
        return Err(LaunchAtLoginError::InvalidExecutablePath(
            "the path contains a quote or control character",
        ));
    }

    let mut value =
        OsString::with_capacity(executable.as_os_str().len() + HIDDEN_LAUNCH_ARGUMENT.len() + 4);
    value.push("\"");
    value.push(executable.as_os_str());
    value.push("\" ");
    value.push(HIDDEN_LAUNCH_ARGUMENT);
    Ok(value)
}

fn registry_add_arguments(launch_value: &OsStr) -> Vec<OsString> {
    [
        "ADD",
        WINDOWS_RUN_KEY,
        "/v",
        WINDOWS_RUN_VALUE_NAME,
        "/t",
        "REG_SZ",
        "/d",
    ]
    .into_iter()
    .map(OsString::from)
    .chain(std::iter::once(launch_value.to_os_string()))
    .chain(std::iter::once(OsString::from("/f")))
    .collect()
}

fn registry_delete_arguments() -> Vec<OsString> {
    [
        "DELETE",
        WINDOWS_RUN_KEY,
        "/v",
        WINDOWS_RUN_VALUE_NAME,
        "/f",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

#[cfg(target_os = "windows")]
fn run_registry_command(
    operation: &'static str,
    arguments: &[OsString],
) -> Result<(), LaunchAtLoginError> {
    let output = std::process::Command::new("reg.exe")
        .args(arguments)
        .output()
        .map_err(LaunchAtLoginError::RegistryUnavailable)?;

    if output.status.success() {
        return Ok(());
    }

    let diagnostic_bytes = if output.stderr.is_empty() {
        output.stdout.as_slice()
    } else {
        output.stderr.as_slice()
    };
    Err(LaunchAtLoginError::RegistryCommandFailed {
        operation,
        exit_code: output.status.code(),
        diagnostic: bounded_registry_diagnostic(diagnostic_bytes),
    })
}

fn bounded_registry_diagnostic(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_REGISTRY_DIAGNOSTIC_BYTES;
    let visible_bytes = &bytes[..bytes.len().min(MAX_REGISTRY_DIAGNOSTIC_BYTES)];
    let diagnostic: String = String::from_utf8_lossy(visible_bytes)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\r' | '\n' | '\t'))
        .collect();
    let diagnostic = diagnostic.trim();

    match (diagnostic.is_empty(), truncated) {
        (true, false) => "no diagnostic output".to_owned(),
        (true, true) => "no readable diagnostic output (truncated)".to_owned(),
        (false, false) => diagnostic.to_owned(),
        (false, true) => format!("{diagnostic} (truncated)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn absolute_test_executable() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            PathBuf::from(r"C:\Program Files\RemoteDeck\RemoteDeck.exe")
        }

        #[cfg(not(target_os = "windows"))]
        {
            PathBuf::from("/opt/Remote Deck/remotedeck")
        }
    }

    #[test]
    fn hidden_launch_requires_an_exact_argument() {
        assert!(is_hidden_launch(["RemoteDeck.exe", "--hidden"]));
        assert!(!is_hidden_launch(["RemoteDeck.exe", "--hidden=true"]));
        assert!(!is_hidden_launch(["RemoteDeck.exe", "prefix--hidden"]));
        assert!(!is_hidden_launch(["RemoteDeck.exe", "--HIDDEN"]));
    }

    #[test]
    fn close_policy_requires_preference_without_a_full_exit() {
        assert!(should_hide_on_close(true, false));
        assert!(!should_hide_on_close(false, false));
        assert!(!should_hide_on_close(true, true));
        assert!(!should_hide_on_close(false, true));
    }

    #[test]
    fn lifecycle_state_records_an_explicit_exit() {
        let lifecycle = LifecycleState::default();
        assert!(!lifecycle.is_full_exit_requested());
        lifecycle.request_full_exit();
        assert!(lifecycle.is_full_exit_requested());
    }

    #[test]
    fn startup_value_quotes_the_executable_and_appends_hidden() {
        let executable = absolute_test_executable();
        let value = build_startup_value(&executable).expect("valid executable path");
        let expected = format!(
            "\"{}\" {HIDDEN_LAUNCH_ARGUMENT}",
            executable.to_string_lossy()
        );
        assert_eq!(value, OsString::from(expected));
    }

    #[test]
    fn startup_value_rejects_relative_and_ambiguous_paths() {
        assert!(matches!(
            build_startup_value(Path::new("RemoteDeck.exe")),
            Err(LaunchAtLoginError::InvalidExecutablePath(_))
        ));

        let mut quoted = absolute_test_executable().into_os_string();
        quoted.push("\"");
        assert!(matches!(
            build_startup_value(Path::new(&quoted)),
            Err(LaunchAtLoginError::InvalidExecutablePath(_))
        ));

        let mut controlled = absolute_test_executable().into_os_string();
        controlled.push("\nsecond.exe");
        assert!(matches!(
            build_startup_value(Path::new(&controlled)),
            Err(LaunchAtLoginError::InvalidExecutablePath(_))
        ));
    }

    #[test]
    fn registry_add_uses_separate_fixed_arguments() {
        let launch_value =
            OsString::from(r#""C:\Program Files\RemoteDeck\RemoteDeck.exe" --hidden"#);
        let arguments = registry_add_arguments(&launch_value);
        let expected = vec![
            OsString::from("ADD"),
            OsString::from(WINDOWS_RUN_KEY),
            OsString::from("/v"),
            OsString::from(WINDOWS_RUN_VALUE_NAME),
            OsString::from("/t"),
            OsString::from("REG_SZ"),
            OsString::from("/d"),
            launch_value,
            OsString::from("/f"),
        ];
        assert_eq!(arguments, expected);
    }

    #[test]
    fn registry_delete_uses_separate_fixed_arguments() {
        let arguments = registry_delete_arguments();
        let expected = [
            "DELETE",
            WINDOWS_RUN_KEY,
            "/v",
            WINDOWS_RUN_VALUE_NAME,
            "/f",
        ]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
        assert_eq!(arguments, expected);
    }

    #[test]
    fn registry_diagnostic_is_bounded_and_removes_unsafe_controls() {
        let mut diagnostic = vec![b'x'; MAX_REGISTRY_DIAGNOSTIC_BYTES + 100];
        diagnostic[0] = 0;
        let result = bounded_registry_diagnostic(&diagnostic);
        assert!(!result.contains('\0'));
        assert!(result.ends_with(" (truncated)"));
        assert!(result.len() <= MAX_REGISTRY_DIAGNOSTIC_BYTES + " (truncated)".len());
    }
}
