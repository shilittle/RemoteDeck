//! Process lifetime, single-instance, and current-user startup support.
//!
//! The local HTTP service is deliberately separate from this module.  This
//! module owns the state that has to outlive a browser tab: Windows logon
//! registration, the compatibility mutex/window, and the small descriptor a
//! second invocation uses to contact the existing loopback service.

use crate::native::{self, NativeError};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ffi::OsStr,
    fmt, fs,
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(any(target_os = "windows", test))]
use std::ffi::OsString;

#[cfg(any(target_os = "windows", test))]
const HIDDEN_LAUNCH_ARGUMENT: &str = "--hidden";
#[cfg(test)]
const STOP_LAUNCH_ARGUMENT: &str = "--stop";

/// The Tauri 2 identifier used by the released desktop application.
#[cfg(target_os = "windows")]
const LEGACY_APPLICATION_IDENTIFIER: &str = "io.github.shilittle.remotedeck";
#[cfg(target_os = "windows")]
const LEGACY_MUTEX_SUFFIX: &str = "-sim";
#[cfg(target_os = "windows")]
const LEGACY_WINDOW_CLASS_SUFFIX: &str = "-sic";
#[cfg(target_os = "windows")]
const LEGACY_WINDOW_NAME_SUFFIX: &str = "-siw";
const RUNTIME_DESCRIPTOR_FILE: &str = "runtime.json";
#[cfg(not(target_os = "windows"))]
const FILE_LOCK_NAME: &str = ".runtime.lock";
const RUNTIME_DESCRIPTOR_SCHEMA_VERSION: u8 = 1;
const MAX_RUNTIME_DESCRIPTOR_BYTES: u64 = 8 * 1024;
const MIN_CONTROL_TOKEN_BYTES: usize = 32;
const MAX_CONTROL_TOKEN_BYTES: usize = 256;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(any(target_os = "windows", test))]
const WINDOWS_RUN_VALUE_NAME: &str = "RemoteDeck";
#[cfg(any(target_os = "windows", test))]
const MAX_REGISTRY_DIAGNOSTIC_BYTES: usize = 4 * 1024;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A descriptor is kept in memory in plaintext only. On Windows its token is
/// DPAPI-protected before it is written, binding it to the current user.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDescriptor {
    pub pid: u32,
    pub base_url: String,
    pub control_token: String,
}

impl fmt::Debug for RuntimeDescriptor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeDescriptor")
            .field("pid", &self.pid)
            .field("base_url", &self.base_url)
            .field("control_token", &"[redacted]")
            .finish()
    }
}

impl RuntimeDescriptor {
    fn validate(&self) -> Result<(), LifecycleError> {
        if self.pid == 0 {
            return Err(LifecycleError::InvalidRuntimeDescriptor(
                "pid must be non-zero",
            ));
        }
        native::validate_loopback_url(&self.base_url).map_err(LifecycleError::Native)?;
        let token = self.control_token.as_bytes();
        if !(MIN_CONTROL_TOKEN_BYTES..=MAX_CONTROL_TOKEN_BYTES).contains(&token.len())
            || !token
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
        {
            return Err(LifecycleError::InvalidRuntimeDescriptor(
                "control token must be a 32-256 byte base64url value",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum LifecycleError {
    InvalidDataDirectory(&'static str),
    InvalidRuntimeDescriptor(&'static str),
    RuntimeDescriptorIo(io::Error),
    RuntimeDescriptorSerialization(serde_json::Error),
    RuntimeDescriptorConflict,
    #[cfg(not(target_os = "windows"))]
    InstanceStarting,
    LegacyInstanceRunning,
    Native(NativeError),
    CurrentExecutable(io::Error),
    InvalidExecutablePath(&'static str),
    RegistryUnavailable(io::Error),
    RegistryCommandFailed {
        operation: &'static str,
        exit_code: Option<i32>,
        diagnostic: String,
    },
    #[cfg(not(target_os = "windows"))]
    UnsupportedPlatform,
    #[cfg(target_os = "windows")]
    WindowsApi(io::Error),
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDataDirectory(reason) => write!(formatter, "invalid data directory: {reason}"),
            Self::InvalidRuntimeDescriptor(reason) => {
                write!(formatter, "invalid local runtime descriptor: {reason}")
            }
            Self::RuntimeDescriptorIo(error) => {
                write!(formatter, "could not access the local runtime descriptor: {error}")
            }
            Self::RuntimeDescriptorSerialization(error) => {
                write!(formatter, "could not parse the local runtime descriptor: {error}")
            }
            Self::RuntimeDescriptorConflict => formatter.write_str(
                "a live RemoteDeck process has a runtime descriptor but does not own its instance lock",
            ),
            #[cfg(not(target_os = "windows"))]
            Self::InstanceStarting => formatter.write_str(
                "another RemoteDeck process is still starting; try again in a moment",
            ),
            Self::LegacyInstanceRunning => formatter.write_str(
                "the legacy RemoteDeck desktop application is running; exit it before starting this version",
            ),
            Self::Native(error) => error.fmt(formatter),
            Self::CurrentExecutable(error) => {
                write!(formatter, "failed to locate the RemoteDeck executable: {error}")
            }
            Self::InvalidExecutablePath(reason) => {
                write!(formatter, "cannot register the RemoteDeck executable: {reason}")
            }
            Self::RegistryUnavailable(error) => {
                write!(formatter, "failed to start the Windows registry tool: {error}")
            }
            Self::RegistryCommandFailed {
                operation,
                exit_code,
                diagnostic,
            } => write!(
                formatter,
                "failed to {operation} launch at login (exit code {exit_code:?}): {diagnostic}"
            ),
            #[cfg(not(target_os = "windows"))]
            Self::UnsupportedPlatform => {
                formatter.write_str("this lifecycle operation is supported only on Windows")
            }
            #[cfg(target_os = "windows")]
            Self::WindowsApi(error) => write!(formatter, "Windows lifecycle API failed: {error}"),
        }
    }
}

impl Error for LifecycleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RuntimeDescriptorIo(error)
            | Self::CurrentExecutable(error)
            | Self::RegistryUnavailable(error) => Some(error),
            Self::RuntimeDescriptorSerialization(error) => Some(error),
            #[cfg(target_os = "windows")]
            Self::WindowsApi(error) => Some(error),
            _ => None,
        }
    }
}

impl From<NativeError> for LifecycleError {
    fn from(error: NativeError) -> Self {
        Self::Native(error)
    }
}

pub enum AcquireResult {
    Primary(RuntimeGuard),
    Existing(RuntimeDescriptor),
}

impl fmt::Debug for AcquireResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primary(_) => formatter.write_str("AcquireResult::Primary(..)"),
            Self::Existing(descriptor) => formatter
                .debug_tuple("AcquireResult::Existing")
                .field(descriptor)
                .finish(),
        }
    }
}

/// Holds this process's single-instance ownership until it exits.
///
/// It is intentionally not cloneable. Dropping it removes only a descriptor
/// published by this process and then releases the platform lock.
pub struct RuntimeGuard {
    data_dir: PathBuf,
    descriptor_path: PathBuf,
    pid: u32,
    published_token: Mutex<Option<String>>,
    #[cfg(target_os = "windows")]
    _legacy_window: LegacyMessageWindow,
    #[cfg(target_os = "windows")]
    _mutex: WindowsMutex,
    #[cfg(not(target_os = "windows"))]
    file_lock: Option<std::fs::File>,
}

impl fmt::Debug for RuntimeGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeGuard")
            .field("data_dir", &self.data_dir)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl RuntimeGuard {
    /// Acquires the current data directory's instance slot.
    ///
    /// The default Tauri data directory deliberately uses its old mutex and
    /// window names. A custom development/test directory receives a stable
    /// path-derived namespace so isolated fixtures can run side by side.
    pub fn acquire(
        data_dir: &Path,
        on_open: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<AcquireResult, LifecycleError> {
        if !data_dir.is_absolute() {
            return Err(LifecycleError::InvalidDataDirectory(
                "the path must be absolute",
            ));
        }
        fs::create_dir_all(data_dir).map_err(LifecycleError::RuntimeDescriptorIo)?;
        let descriptor_path = runtime_descriptor_path(data_dir);
        let pid = std::process::id();

        #[cfg(target_os = "windows")]
        {
            let namespace = instance_namespace(data_dir);
            match WindowsMutex::acquire(&namespace.mutex_name)? {
                WindowsMutexAcquire::Existing => match read_runtime_descriptor(data_dir) {
                    Ok(Some(descriptor)) if is_process_alive(descriptor.pid) => {
                        Ok(AcquireResult::Existing(descriptor))
                    }
                    // An existing legacy mutex has no descriptor. A corrupt
                    // or stale descriptor is also not evidence that the
                    // currently-locking process is our new server.
                    _ => Err(LifecycleError::LegacyInstanceRunning),
                },
                WindowsMutexAcquire::Primary(mutex) => {
                    cleanup_stale_descriptor(&descriptor_path)?;
                    let legacy_window = LegacyMessageWindow::start(
                        namespace.window_class,
                        namespace.window_name,
                        on_open,
                    )?;
                    Ok(AcquireResult::Primary(Self {
                        data_dir: data_dir.to_path_buf(),
                        descriptor_path,
                        pid,
                        published_token: Mutex::new(None),
                        _mutex: mutex,
                        _legacy_window: legacy_window,
                    }))
                }
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = on_open;
            match acquire_file_lock(data_dir, &descriptor_path)? {
                FileLockAcquire::Existing => match read_runtime_descriptor(data_dir) {
                    Ok(Some(descriptor)) if is_process_alive(descriptor.pid) => {
                        Ok(AcquireResult::Existing(descriptor))
                    }
                    _ => Err(LifecycleError::InstanceStarting),
                },
                FileLockAcquire::Primary(file_lock) => {
                    cleanup_stale_descriptor(&descriptor_path)?;
                    Ok(AcquireResult::Primary(Self {
                        data_dir: data_dir.to_path_buf(),
                        descriptor_path,
                        pid,
                        published_token: Mutex::new(None),
                        file_lock: Some(file_lock),
                    }))
                }
            }
        }
    }

    /// Atomically publishes the local service endpoint after it has bound.
    pub fn publish(&self, descriptor: RuntimeDescriptor) -> Result<(), LifecycleError> {
        if descriptor.pid != self.pid {
            return Err(LifecycleError::InvalidRuntimeDescriptor(
                "descriptor pid does not identify this process",
            ));
        }
        descriptor.validate()?;
        write_runtime_descriptor(&self.descriptor_path, &descriptor)?;
        let mut published = self
            .published_token
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *published = Some(descriptor.control_token);
        Ok(())
    }
}

impl Drop for RuntimeGuard {
    fn drop(&mut self) {
        let token = self
            .published_token
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(token) = token
            && let Ok(Some(descriptor)) = read_runtime_descriptor(&self.data_dir)
            && descriptor.pid == self.pid
            && descriptor.control_token == token
        {
            let _ = fs::remove_file(&self.descriptor_path);
        }

        #[cfg(not(target_os = "windows"))]
        {
            self.file_lock.take();
            let lock_path = self.data_dir.join(FILE_LOCK_NAME);
            if lock_belongs_to_process(&lock_path, self.pid) {
                let _ = fs::remove_file(lock_path);
            }
        }
    }
}

/// Reads a complete, validated descriptor for the current Windows user.
fn read_runtime_descriptor(data_dir: &Path) -> Result<Option<RuntimeDescriptor>, LifecycleError> {
    let path = runtime_descriptor_path(data_dir);
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(LifecycleError::RuntimeDescriptorIo(error)),
    };
    if metadata.len() > MAX_RUNTIME_DESCRIPTOR_BYTES {
        return Err(LifecycleError::InvalidRuntimeDescriptor(
            "file exceeds the size limit",
        ));
    }
    let bytes = fs::read(&path).map_err(LifecycleError::RuntimeDescriptorIo)?;
    let disk: DiskRuntimeDescriptor =
        serde_json::from_slice(&bytes).map_err(LifecycleError::RuntimeDescriptorSerialization)?;
    if disk.schema_version != RUNTIME_DESCRIPTOR_SCHEMA_VERSION {
        return Err(LifecycleError::InvalidRuntimeDescriptor(
            "unsupported schema version",
        ));
    }
    let descriptor = RuntimeDescriptor {
        pid: disk.pid,
        base_url: disk.base_url,
        control_token: decode_control_token(&disk.control_token)?,
    };
    descriptor.validate()?;
    Ok(Some(descriptor))
}

fn runtime_descriptor_path(data_dir: &Path) -> PathBuf {
    data_dir.join(RUNTIME_DESCRIPTOR_FILE)
}

/// Checks whether a process ID is currently alive without sending it a signal.
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, STILL_ACTIVE},
            System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        };

        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return false;
        }
        let mut exit_code = 0;
        let result = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0;
        unsafe {
            CloseHandle(handle);
        }
        result && exit_code == STILL_ACTIVE as u32
    }

    #[cfg(not(target_os = "windows"))]
    {
        Path::new("/proc").join(pid.to_string()).is_dir()
    }
}

/// Returns `true` only when an exact argument is present.
#[cfg(test)]
fn has_launch_argument<I, S>(arguments: I, expected: &str) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    arguments
        .into_iter()
        .any(|argument| argument.as_ref() == OsStr::new(expected))
}

/// Enables or disables the per-user Windows logon entry.
///
/// `reg.exe` receives fixed, separate arguments. No command shell is involved,
/// so metacharacters in an installation path cannot be interpreted.
pub fn set_launch_at_login(enabled: bool) -> Result<(), LifecycleError> {
    #[cfg(target_os = "windows")]
    {
        let (operation, arguments) = if enabled {
            let executable = std::env::current_exe().map_err(LifecycleError::CurrentExecutable)?;
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
        Err(LifecycleError::UnsupportedPlatform)
    }
}

#[cfg(any(target_os = "windows", test))]
fn build_startup_value(executable: &Path) -> Result<OsString, LifecycleError> {
    if !executable.is_absolute() {
        return Err(LifecycleError::InvalidExecutablePath(
            "the path is not absolute",
        ));
    }
    if executable.file_name().is_none() {
        return Err(LifecycleError::InvalidExecutablePath(
            "the path does not identify a file",
        ));
    }
    let executable_text = executable
        .to_str()
        .ok_or(LifecycleError::InvalidExecutablePath(
            "the path is not valid Unicode",
        ))?;
    if executable_text
        .chars()
        .any(|character| character == '"' || character.is_control())
    {
        return Err(LifecycleError::InvalidExecutablePath(
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

#[cfg(any(target_os = "windows", test))]
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

#[cfg(any(target_os = "windows", test))]
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
) -> Result<(), LifecycleError> {
    let output = remotedeck_core::process::command("reg.exe")
        .args(arguments)
        .output()
        .map_err(LifecycleError::RegistryUnavailable)?;
    if output.status.success() {
        return Ok(());
    }
    let diagnostic_bytes = if output.stderr.is_empty() {
        output.stdout.as_slice()
    } else {
        output.stderr.as_slice()
    };
    Err(LifecycleError::RegistryCommandFailed {
        operation,
        exit_code: output.status.code(),
        diagnostic: bounded_registry_diagnostic(diagnostic_bytes),
    })
}

#[cfg(any(target_os = "windows", test))]
fn bounded_registry_diagnostic(bytes: &[u8]) -> String {
    let truncated = bytes.len() > MAX_REGISTRY_DIAGNOSTIC_BYTES;
    let visible = &bytes[..bytes.len().min(MAX_REGISTRY_DIAGNOSTIC_BYTES)];
    let diagnostic: String = String::from_utf8_lossy(visible)
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiskRuntimeDescriptor {
    schema_version: u8,
    pid: u32,
    base_url: String,
    control_token: String,
}

fn write_runtime_descriptor(
    path: &Path,
    descriptor: &RuntimeDescriptor,
) -> Result<(), LifecycleError> {
    let disk = DiskRuntimeDescriptor {
        schema_version: RUNTIME_DESCRIPTOR_SCHEMA_VERSION,
        pid: descriptor.pid,
        base_url: descriptor.base_url.clone(),
        control_token: encode_control_token(&descriptor.control_token)?,
    };
    let contents =
        serde_json::to_vec(&disk).map_err(LifecycleError::RuntimeDescriptorSerialization)?;
    let parent = path.parent().ok_or(LifecycleError::InvalidDataDirectory(
        "runtime descriptor has no parent",
    ))?;
    fs::create_dir_all(parent).map_err(LifecycleError::RuntimeDescriptorIo)?;

    let unique = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".runtime-{}-{timestamp}-{unique}.tmp",
        std::process::id()
    ));
    let result = (|| -> Result<(), LifecycleError> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(LifecycleError::RuntimeDescriptorIo)?;
        file.write_all(&contents)
            .and_then(|_| file.sync_all())
            .map_err(LifecycleError::RuntimeDescriptorIo)?;
        drop(file);
        fs::rename(&temporary, path).map_err(LifecycleError::RuntimeDescriptorIo)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn cleanup_stale_descriptor(path: &Path) -> Result<(), LifecycleError> {
    let data_dir = path.parent().ok_or(LifecycleError::InvalidDataDirectory(
        "runtime descriptor has no parent",
    ))?;
    match read_runtime_descriptor(data_dir)? {
        Some(descriptor) if is_process_alive(descriptor.pid) => {
            Err(LifecycleError::RuntimeDescriptorConflict)
        }
        Some(_) => fs::remove_file(path).map_err(LifecycleError::RuntimeDescriptorIo),
        None => Ok(()),
    }
}

#[cfg(target_os = "windows")]
fn encode_control_token(value: &str) -> Result<String, LifecycleError> {
    dpapi_protect(value.as_bytes()).map(|bytes| hex_encode(&bytes))
}

#[cfg(not(target_os = "windows"))]
fn encode_control_token(value: &str) -> Result<String, LifecycleError> {
    Ok(value.to_owned())
}

#[cfg(target_os = "windows")]
fn decode_control_token(value: &str) -> Result<String, LifecycleError> {
    let bytes = hex_decode(value)?;
    let plaintext = dpapi_unprotect(&bytes)?;
    String::from_utf8(plaintext)
        .map_err(|_| LifecycleError::InvalidRuntimeDescriptor("DPAPI token is not UTF-8"))
}

#[cfg(not(target_os = "windows"))]
fn decode_control_token(value: &str) -> Result<String, LifecycleError> {
    Ok(value.to_owned())
}

#[cfg(target_os = "windows")]
fn dpapi_protect(input: &[u8]) -> Result<Vec<u8>, LifecycleError> {
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };

    let mut input = input.to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len())
            .map_err(|_| LifecycleError::InvalidRuntimeDescriptor("control token is too large"))?,
        pbData: input.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let protected = unsafe {
        CryptProtectData(
            &input_blob,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if protected == 0 {
        return Err(LifecycleError::WindowsApi(io::Error::last_os_error()));
    }
    unsafe { take_windows_blob(output) }
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(input: &[u8]) -> Result<Vec<u8>, LifecycleError> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };

    let mut input = input.to_vec();
    let input_blob = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(input.len()).map_err(|_| {
            LifecycleError::InvalidRuntimeDescriptor("protected token is too large")
        })?,
        pbData: input.as_mut_ptr(),
    };
    let mut description = std::ptr::null_mut();
    let mut output = CRYPT_INTEGER_BLOB::default();
    let unprotected = unsafe {
        CryptUnprotectData(
            &input_blob,
            &mut description,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if !description.is_null() {
        unsafe {
            LocalFree(description.cast());
        }
    }
    if unprotected == 0 {
        return Err(LifecycleError::WindowsApi(io::Error::last_os_error()));
    }
    unsafe { take_windows_blob(output) }
}

#[cfg(target_os = "windows")]
unsafe fn take_windows_blob(
    blob: windows_sys::Win32::Security::Cryptography::CRYPT_INTEGER_BLOB,
) -> Result<Vec<u8>, LifecycleError> {
    use windows_sys::Win32::Foundation::LocalFree;
    let bytes = if blob.cbData == 0 || blob.pbData.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec() }
    };
    unsafe {
        LocalFree(blob.pbData.cast());
    }
    Ok(bytes)
}

#[cfg(target_os = "windows")]
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(target_os = "windows")]
fn hex_decode(value: &str) -> Result<Vec<u8>, LifecycleError> {
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || value.len() > MAX_RUNTIME_DESCRIPTOR_BYTES as usize
    {
        return Err(LifecycleError::InvalidRuntimeDescriptor(
            "protected token is malformed",
        ));
    }
    fn nibble(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| match (nibble(pair[0]), nibble(pair[1])) {
            (Some(high), Some(low)) => Ok((high << 4) | low),
            _ => Err(LifecycleError::InvalidRuntimeDescriptor(
                "protected token is malformed",
            )),
        })
        .collect()
}

#[cfg(target_os = "windows")]
struct InstanceNamespace {
    mutex_name: String,
    window_class: String,
    window_name: String,
}

#[cfg(target_os = "windows")]
fn instance_namespace(data_dir: &Path) -> InstanceNamespace {
    let identifier = if is_default_data_dir(data_dir) {
        LEGACY_APPLICATION_IDENTIFIER.to_owned()
    } else {
        format!(
            "{LEGACY_APPLICATION_IDENTIFIER}.server.{:016x}",
            stable_path_hash(data_dir)
        )
    };
    InstanceNamespace {
        mutex_name: format!("{identifier}{LEGACY_MUTEX_SUFFIX}"),
        window_class: format!("{identifier}{LEGACY_WINDOW_CLASS_SUFFIX}"),
        window_name: format!("{identifier}{LEGACY_WINDOW_NAME_SUFFIX}"),
    }
}

#[cfg(target_os = "windows")]
fn is_default_data_dir(data_dir: &Path) -> bool {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return false;
    };
    paths_equal(
        data_dir,
        &PathBuf::from(appdata).join(LEGACY_APPLICATION_IDENTIFIER),
    )
}

#[cfg(target_os = "windows")]
fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

#[cfg(any(target_os = "windows", test))]
fn stable_path_hash(path: &Path) -> u64 {
    // FNV-1a avoids the process-random seed used by DefaultHasher. This is an
    // instance namespace, not a security primitive; it only needs stability.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.to_string_lossy().bytes() {
        hash ^= u64::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(target_os = "windows")]
enum WindowsMutexAcquire {
    Primary(WindowsMutex),
    Existing,
}

#[cfg(target_os = "windows")]
struct WindowsMutex {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(target_os = "windows")]
impl WindowsMutex {
    fn acquire(name: &str) -> Result<WindowsMutexAcquire, LifecycleError> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError},
            System::Threading::CreateMutexW,
        };

        let wide: Vec<u16> = OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, wide.as_ptr()) };
        if handle.is_null() {
            return Err(LifecycleError::WindowsApi(io::Error::last_os_error()));
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            return Ok(WindowsMutexAcquire::Existing);
        }
        Ok(WindowsMutexAcquire::Primary(Self { handle }))
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsMutex {
    fn drop(&mut self) {
        use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::ReleaseMutex};
        unsafe {
            ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

#[cfg(target_os = "windows")]
struct LegacyMessageWindow {
    hwnd: isize,
    message_thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
impl LegacyMessageWindow {
    fn start(
        class_name: String,
        window_name: String,
        on_open: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, LifecycleError> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let message_thread = std::thread::Builder::new()
            .name("remotedeck-legacy-window".to_owned())
            .spawn(move || run_legacy_message_window(class_name, window_name, on_open, sender))
            .map_err(LifecycleError::WindowsApi)?;
        let hwnd = receiver.recv().map_err(|_| {
            LifecycleError::WindowsApi(io::Error::other("legacy window thread stopped"))
        })??;
        Ok(Self {
            hwnd,
            message_thread: Some(message_thread),
        })
    }
}

#[cfg(target_os = "windows")]
impl Drop for LegacyMessageWindow {
    fn drop(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        let posted =
            unsafe { PostMessageW(self.hwnd as *mut std::ffi::c_void, WM_CLOSE, 0, 0) } != 0;
        if posted && let Some(message_thread) = self.message_thread.take() {
            let _ = message_thread.join();
        }
    }
}

#[cfg(target_os = "windows")]
struct LegacyWindowUserData {
    on_open: Arc<dyn Fn() + Send + Sync>,
}

#[cfg(target_os = "windows")]
fn run_legacy_message_window(
    class_name: String,
    window_name: String,
    on_open: Arc<dyn Fn() + Send + Sync>,
    sender: std::sync::mpsc::SyncSender<Result<isize, LifecycleError>>,
) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW,
            TranslateMessage, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
        },
    };

    let class_name: Vec<u16> = OsStr::new(&class_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let window_name: Vec<u16> = OsStr::new(&window_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let module = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(legacy_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: module,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    let registered = unsafe { RegisterClassExW(&class) };
    if registered == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
        let _ = sender.send(Err(LifecycleError::WindowsApi(io::Error::last_os_error())));
        return;
    }

    let user_data = Box::into_raw(Box::new(LegacyWindowUserData { on_open }));
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            window_name.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            module,
            user_data.cast(),
        )
    };
    if hwnd.is_null() {
        unsafe {
            drop(Box::from_raw(user_data));
        }
        let _ = sender.send(Err(LifecycleError::WindowsApi(io::Error::last_os_error())));
        return;
    }
    if sender.send(Ok(hwnd as isize)).is_err() {
        // The owner went away before the thread could be retained. Destroying
        // the window releases the callback allocation through WM_DESTROY.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
        }
        return;
    }

    let mut message = MSG::default();
    while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn legacy_window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    _wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, PostQuitMessage,
        SetWindowLongPtrW, WM_COPYDATA, WM_CREATE, WM_DESTROY,
    };

    match message {
        WM_CREATE => {
            let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            }
            0
        }
        WM_COPYDATA => {
            // The legacy process passes a cwd/argument payload. It is not
            // trusted or parsed: the only result is opening this service's
            // already-configured loopback URL.
            let pointer =
                unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const LegacyWindowUserData;
            if !pointer.is_null() {
                let callback = unsafe { &(*pointer).on_open };
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback()));
            }
            1
        }
        WM_DESTROY => {
            let pointer =
                unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut LegacyWindowUserData;
            if !pointer.is_null() {
                unsafe {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(pointer));
                }
            }
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, _wparam, lparam) },
    }
}

#[cfg(not(target_os = "windows"))]
enum FileLockAcquire {
    Primary(std::fs::File),
    Existing,
}

#[cfg(not(target_os = "windows"))]
fn acquire_file_lock(
    data_dir: &Path,
    descriptor_path: &Path,
) -> Result<FileLockAcquire, LifecycleError> {
    let path = data_dir.join(FILE_LOCK_NAME);
    for _ in 0..2 {
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                file.write_all(std::process::id().to_string().as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(LifecycleError::RuntimeDescriptorIo)?;
                return Ok(FileLockAcquire::Primary(file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if lock_owner_is_alive(&path) {
                    return Ok(FileLockAcquire::Existing);
                }
                // A crashed Linux CI process can leave a marker. Only remove it
                // after its recorded PID is known dead; never use this fallback
                // as a production multi-user locking mechanism.
                match fs::remove_file(&path) {
                    Ok(()) => continue,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(LifecycleError::RuntimeDescriptorIo(error)),
                }
            }
            Err(error) => return Err(LifecycleError::RuntimeDescriptorIo(error)),
        }
    }
    let _ = descriptor_path;
    Err(LifecycleError::InstanceStarting)
}

#[cfg(not(target_os = "windows"))]
fn lock_owner_is_alive(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .is_some_and(is_process_alive)
}

#[cfg(not(target_os = "windows"))]
fn lock_belongs_to_process(path: &Path, pid: u32) -> bool {
    fs::read_to_string(path)
        .map(|value| value.trim() == pid.to_string())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(pid: u32) -> RuntimeDescriptor {
        RuntimeDescriptor {
            pid,
            base_url: "http://127.0.0.1:49152".to_owned(),
            control_token: "a".repeat(MIN_CONTROL_TOKEN_BYTES),
        }
    }

    fn unique_data_dir(name: &str) -> PathBuf {
        let unique = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "remotedeck-server-lifecycle-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn descriptor_validation_requires_a_safe_loopback_origin_and_token() {
        assert!(descriptor(1).validate().is_ok());
        let mut invalid = descriptor(1);
        invalid.base_url = "http://example.test:49152".to_owned();
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor(1);
        invalid.control_token = "short".to_owned();
        assert!(invalid.validate().is_err());
        let mut invalid = descriptor(1);
        invalid.control_token = "a".repeat(MIN_CONTROL_TOKEN_BYTES - 1) + "!";
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn descriptor_round_trip_and_guard_drop_remove_only_its_file() {
        let data_dir = unique_data_dir("round-trip");
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let guard = match RuntimeGuard::acquire(&data_dir, callback).expect("acquire") {
            AcquireResult::Primary(guard) => guard,
            AcquireResult::Existing(_) => panic!("unexpected existing instance"),
        };
        let descriptor = descriptor(std::process::id());
        guard.publish(descriptor.clone()).expect("publish");
        #[cfg(target_os = "windows")]
        assert!(
            !fs::read_to_string(runtime_descriptor_path(&data_dir))
                .expect("descriptor file")
                .contains(&descriptor.control_token),
            "the on-disk descriptor must not expose its control token"
        );
        assert_eq!(
            read_runtime_descriptor(&data_dir).expect("read"),
            Some(descriptor)
        );
        drop(guard);
        assert!(!runtime_descriptor_path(&data_dir).exists());
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn a_second_acquire_returns_the_live_descriptor() {
        let data_dir = unique_data_dir("existing");
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let guard = match RuntimeGuard::acquire(&data_dir, callback.clone()).expect("first acquire")
        {
            AcquireResult::Primary(guard) => guard,
            AcquireResult::Existing(_) => panic!("unexpected existing instance"),
        };
        let descriptor = descriptor(std::process::id());
        guard.publish(descriptor.clone()).expect("publish");
        match RuntimeGuard::acquire(&data_dir, callback).expect("second acquire") {
            AcquireResult::Existing(found) => assert_eq!(found, descriptor),
            AcquireResult::Primary(_) => panic!("second acquire unexpectedly became primary"),
        }
        drop(guard);
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn stale_metadata_is_removed_before_a_new_primary_starts() {
        let data_dir = unique_data_dir("stale");
        fs::create_dir_all(&data_dir).expect("data directory");
        let stale = descriptor(u32::MAX);
        write_runtime_descriptor(&runtime_descriptor_path(&data_dir), &stale).expect("write stale");
        let callback: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let guard = match RuntimeGuard::acquire(&data_dir, callback).expect("acquire") {
            AcquireResult::Primary(guard) => guard,
            AcquireResult::Existing(_) => panic!("stale descriptor is not an instance"),
        };
        assert!(!runtime_descriptor_path(&data_dir).exists());
        drop(guard);
        let _ = fs::remove_dir_all(&data_dir);
    }

    #[test]
    fn process_liveness_recognizes_this_process_and_rejects_zero() {
        assert!(is_process_alive(std::process::id()));
        assert!(!is_process_alive(0));
    }

    #[test]
    fn launch_arguments_require_exact_values() {
        assert!(has_launch_argument(
            ["RemoteDeck.exe", "--hidden"],
            HIDDEN_LAUNCH_ARGUMENT
        ));
        assert!(!has_launch_argument(
            ["RemoteDeck.exe", "--hidden=true"],
            HIDDEN_LAUNCH_ARGUMENT
        ));
        assert!(!has_launch_argument(
            ["RemoteDeck.exe", "prefix--stop"],
            STOP_LAUNCH_ARGUMENT
        ));
    }

    #[test]
    fn startup_value_quotes_the_executable_and_uses_hidden_mode() {
        let executable = if cfg!(target_os = "windows") {
            PathBuf::from(r"C:\Program Files\RemoteDeck\RemoteDeck.exe")
        } else {
            PathBuf::from("/opt/Remote Deck/remotedeck")
        };
        let value = build_startup_value(&executable).expect("valid path");
        assert_eq!(
            value,
            OsString::from(format!(
                "\"{}\" {HIDDEN_LAUNCH_ARGUMENT}",
                executable.display()
            ))
        );
    }

    #[test]
    fn startup_value_rejects_relative_or_ambiguous_paths() {
        assert!(build_startup_value(Path::new("RemoteDeck.exe")).is_err());
        let mut quoted = if cfg!(target_os = "windows") {
            OsString::from(r"C:\Program Files\RemoteDeck\RemoteDeck.exe")
        } else {
            OsString::from("/opt/remotedeck")
        };
        quoted.push("\"");
        assert!(build_startup_value(Path::new(&quoted)).is_err());
    }

    #[test]
    fn registry_arguments_are_fixed_and_shell_free() {
        let launch_value =
            OsString::from(r#""C:\Program Files\RemoteDeck\RemoteDeck.exe" --hidden"#);
        let arguments = registry_add_arguments(&launch_value);
        assert_eq!(arguments[0], OsString::from("ADD"));
        assert_eq!(arguments[1], OsString::from(WINDOWS_RUN_KEY));
        assert_eq!(arguments[7], launch_value);
        assert_eq!(registry_delete_arguments()[0], OsString::from("DELETE"));
    }

    #[test]
    fn registry_diagnostics_are_bounded_and_strip_controls() {
        let mut bytes = vec![b'x'; MAX_REGISTRY_DIAGNOSTIC_BYTES + 1];
        bytes[0] = 0;
        let diagnostic = bounded_registry_diagnostic(&bytes);
        assert!(!diagnostic.contains('\0'));
        assert!(diagnostic.ends_with(" (truncated)"));
    }

    #[test]
    fn path_hash_is_stable() {
        let path = Path::new("/tmp/remotedeck-test");
        assert_eq!(stable_path_hash(path), stable_path_hash(path));
        assert_ne!(
            stable_path_hash(path),
            stable_path_hash(Path::new("/tmp/remotedeck-other"))
        );
    }
}
