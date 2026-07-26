mod error;
mod model;
mod session;
mod ssh;
mod store;
mod tunnel;

use tauri::{AppHandle, Manager, RunEvent, State};
use crate::{
    error::AppResult,
    model::{AppSettings, BootstrapPayload, CommandResult, ConnectionTestResult, HostDraft, HostKeyCandidate, HostProfile, SettingsPatch, TerminalSnapshot, TunnelDraft, TunnelProfile, TunnelSnapshot},
    session::TerminalRegistry, ssh::SshRuntime, store::AppRepository, tunnel::TunnelRegistry,
};

struct AppState { repository: AppRepository, ssh: SshRuntime, terminals: TerminalRegistry, tunnels: TunnelRegistry }

#[tauri::command]
fn bootstrap(state: State<'_, AppState>) -> BootstrapPayload {
    let snapshot = state.repository.snapshot();
    BootstrapPayload { app_version: env!("CARGO_PKG_VERSION").to_owned(), settings: snapshot.settings, hosts: snapshot.hosts, tunnels: snapshot.tunnels, capabilities: state.ssh.capabilities() }
}
#[tauri::command]
fn save_host(state: State<'_, AppState>, draft: HostDraft) -> AppResult<HostProfile> { state.repository.save_host(draft) }
#[tauri::command]
fn delete_host(app: AppHandle, state: State<'_, AppState>, host_id: String) -> AppResult<()> { state.terminals.stop_for_host(&host_id); state.tunnels.stop_for_host(&app, &host_id); state.repository.delete_host(&host_id) }
#[tauri::command]
async fn scan_host_keys(app: AppHandle, host_id: String) -> AppResult<Vec<HostKeyCandidate>> {
    let (host, ssh) = { let state = app.state::<AppState>(); (state.repository.host(&host_id)?, state.ssh.clone()) }; ssh.scan_host_keys(&host).await
}
#[tauri::command]
async fn accept_host_key(app: AppHandle, host_id: String, candidate: HostKeyCandidate) -> AppResult<()> {
    let (host, ssh) = { let state = app.state::<AppState>(); (state.repository.host(&host_id)?, state.ssh.clone()) }; ssh.accept_host_key(&host, &candidate).await
}
#[tauri::command]
async fn test_connection(app: AppHandle, host_id: String) -> AppResult<ConnectionTestResult> {
    let (host, ssh) = { let state = app.state::<AppState>(); (state.repository.host(&host_id)?, state.ssh.clone()) }; ssh.test_connection(&host).await
}
#[tauri::command]
async fn run_command(app: AppHandle, host_id: String, command: String, working_directory: Option<String>) -> AppResult<CommandResult> {
    let (host, ssh) = { let state = app.state::<AppState>(); (state.repository.host(&host_id)?, state.ssh.clone()) }; ssh.run_command(&host, command, working_directory).await
}
#[tauri::command]
fn start_terminal(app: AppHandle, state: State<'_, AppState>, host_id: String, rows: u16, cols: u16) -> AppResult<TerminalSnapshot> {
    let host = state.repository.host(&host_id)?; state.terminals.start(app, &host, &state.ssh, rows, cols)
}
#[tauri::command]
fn write_terminal(state: State<'_, AppState>, session_id: String, data: String) -> AppResult<()> { state.terminals.write(&session_id, &data) }
#[tauri::command]
fn resize_terminal(state: State<'_, AppState>, session_id: String, rows: u16, cols: u16) -> AppResult<()> { state.terminals.resize(&session_id, rows, cols) }
#[tauri::command]
fn close_terminal(state: State<'_, AppState>, session_id: String) -> AppResult<()> { state.terminals.close(&session_id) }
#[tauri::command]
fn save_tunnel(state: State<'_, AppState>, draft: TunnelDraft) -> AppResult<TunnelProfile> { state.repository.save_tunnel(draft) }
#[tauri::command]
fn delete_tunnel(app: AppHandle, state: State<'_, AppState>, tunnel_id: String) -> AppResult<()> { let _ = state.tunnels.stop(&app, &tunnel_id); state.repository.delete_tunnel(&tunnel_id) }
#[tauri::command]
fn start_tunnel(app: AppHandle, state: State<'_, AppState>, tunnel_id: String) -> AppResult<TunnelSnapshot> {
    let tunnel = state.repository.tunnel(&tunnel_id)?; let host = state.repository.host(&tunnel.host_id)?; state.tunnels.start(app, &host, &tunnel, &state.ssh)
}
#[tauri::command]
fn stop_tunnel(app: AppHandle, state: State<'_, AppState>, tunnel_id: String) -> AppResult<()> { state.tunnels.stop(&app, &tunnel_id) }
#[tauri::command]
fn update_settings(state: State<'_, AppState>, patch: SettingsPatch) -> AppResult<AppSettings> { state.repository.update_settings(patch) }

pub fn run() {
    let application = tauri::Builder::default()
        .setup(|app| {
            let repository = AppRepository::open(app.path().app_data_dir()?)?;
            let ssh = SshRuntime::discover(repository.known_hosts_path().to_path_buf());
            app.manage(AppState { repository, ssh, terminals: TerminalRegistry::default(), tunnels: TunnelRegistry::default() });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![bootstrap, save_host, delete_host, scan_host_keys, accept_host_key, test_connection, run_command, start_terminal, write_terminal, resize_terminal, close_terminal, save_tunnel, delete_tunnel, start_tunnel, stop_tunnel, update_settings])
        .build(tauri::generate_context!())
        .expect("failed to build RemoteDeck");
    application.run(|app, event| {
        if matches!(event, RunEvent::Exit) { let state = app.state::<AppState>(); state.terminals.stop_all(); state.tunnels.stop_all(app); }
    });
}
