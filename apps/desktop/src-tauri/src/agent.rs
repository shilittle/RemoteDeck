//! Product-neutral command planning for interactive coding-agent CLIs.
//!
//! Plans are inert data.  This module never spawns a process, reads credential
//! files, accepts API tokens, or parses a provider's TUI output.  Installation
//! plans are always marked as requiring explicit review and confirmation.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};
use uuid::Uuid;

const MAX_WORKSPACE_BYTES: usize = 4096;
const MAX_SESSION_SELECTOR_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentProvider {
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "claude", alias = "claude_code")]
    ClaudeCode,
    #[serde(rename = "gemini", alias = "gemini_cli")]
    GeminiCli,
    #[serde(rename = "opencode")]
    OpenCode,
}

impl AgentProvider {
    #[cfg(test)]
    pub const ALL: [Self; 4] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::GeminiCli,
        Self::OpenCode,
    ];

    pub const fn executable(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude",
            Self::GeminiCli => "gemini",
            Self::OpenCode => "opencode",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "OpenAI Codex CLI",
            Self::ClaudeCode => "Claude Code",
            Self::GeminiCli => "Gemini CLI",
            Self::OpenCode => "OpenCode",
        }
    }

    pub const fn tmux_session_prefix(self) -> &'static str {
        match self {
            Self::Codex => "remotedeck-codex-",
            Self::ClaudeCode => "remotedeck-claude-",
            Self::GeminiCli => "remotedeck-gemini-",
            Self::OpenCode => "remotedeck-opencode-",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAction {
    Probe,
    Install,
    Login,
    Update,
    Start,
    Resume,
}

impl AgentAction {
    #[cfg(test)]
    pub const ALL: [Self; 6] = [
        Self::Probe,
        Self::Install,
        Self::Login,
        Self::Update,
        Self::Start,
        Self::Resume,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    Capture,
    InteractivePty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPlanRequest {
    pub provider: AgentProvider,
    pub action: AgentAction,
    pub host_id: String,
    pub workspace: String,
    #[serde(default)]
    pub resume_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedCommand {
    pub command: String,
    pub mode: PlanMode,
    pub purpose: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandPlan {
    pub provider: AgentProvider,
    pub action: AgentAction,
    pub display_name: String,
    pub commands: Vec<PlannedCommand>,
    pub requires_confirmation: bool,
    pub source_url: Option<String>,
    pub tmux_session: Option<String>,
    pub notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPlanError {
    InvalidHostId,
    InvalidWorkspace,
    InvalidSessionSelector,
    UnownedSessionSelector,
    UnexpectedSessionSelector,
    UnsafeGeneratedPlan,
}

impl fmt::Display for AgentPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHostId => formatter.write_str("host ID must be a UUID"),
            Self::InvalidWorkspace => {
                formatter.write_str("workspace is empty, oversized, or contains control characters")
            }
            Self::InvalidSessionSelector => formatter.write_str(
                "session selector must contain only ASCII letters, digits, dot, underscore, colon, or dash",
            ),
            Self::UnownedSessionSelector => formatter.write_str(
                "session selector is not a RemoteDeck-owned tmux session for this provider",
            ),
            Self::UnexpectedSessionSelector => {
                formatter.write_str("a session selector is valid only for resume plans")
            }
            Self::UnsafeGeneratedPlan => {
                formatter.write_str("generated plan contains a permission-bypass option")
            }
        }
    }
}

impl Error for AgentPlanError {}

/// Returns a validated, inert command plan for the requested provider action.
pub fn plan_agent_action(request: &AgentPlanRequest) -> Result<AgentCommandPlan, AgentPlanError> {
    validate_request(request)?;
    let mut plan = match request.action {
        AgentAction::Probe => probe_plan(request.provider),
        AgentAction::Install => install_plan(request.provider),
        AgentAction::Login => login_plan(request.provider, &request.workspace),
        AgentAction::Update => update_plan(request.provider, &request.workspace),
        AgentAction::Start => start_plan(request),
        AgentAction::Resume => resume_plan(request),
    };
    ensure_plan_is_safe(&plan)?;
    plan.display_name = request.provider.display_name().to_owned();
    Ok(plan)
}

/// Stable provider-specific tmux association derived from host and workspace identity.
pub fn stable_agent_tmux_session(
    provider: AgentProvider,
    host_id: &str,
    workspace: &str,
) -> Result<String, AgentPlanError> {
    Uuid::parse_str(host_id).map_err(|_| AgentPlanError::InvalidHostId)?;
    validate_workspace(workspace)?;
    let digest = Sha256::digest([host_id.as_bytes(), b"\0", workspace.as_bytes()].concat());
    let suffix = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("{}{suffix}", provider.tmux_session_prefix()))
}

pub fn is_owned_tmux_session(provider: AgentProvider, value: &str) -> bool {
    value
        .strip_prefix(provider.tmux_session_prefix())
        .is_some_and(|suffix| !suffix.is_empty())
        && validate_session_selector(value).is_ok()
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn validate_request(request: &AgentPlanRequest) -> Result<(), AgentPlanError> {
    Uuid::parse_str(&request.host_id).map_err(|_| AgentPlanError::InvalidHostId)?;
    validate_workspace(&request.workspace)?;
    if let Some(selector) = request.resume_session.as_deref() {
        if request.action != AgentAction::Resume {
            return Err(AgentPlanError::UnexpectedSessionSelector);
        }
        validate_session_selector(selector)?;
        if !is_owned_tmux_session(request.provider, selector) {
            return Err(AgentPlanError::UnownedSessionSelector);
        }
    }
    Ok(())
}

fn validate_workspace(value: &str) -> Result<(), AgentPlanError> {
    if value.trim().is_empty()
        || value.len() > MAX_WORKSPACE_BYTES
        || value
            .chars()
            .any(|character| character == '\0' || character == '\r' || character == '\n')
    {
        return Err(AgentPlanError::InvalidWorkspace);
    }
    Ok(())
}

fn validate_session_selector(value: &str) -> Result<(), AgentPlanError> {
    if value.is_empty()
        || value.len() > MAX_SESSION_SELECTOR_BYTES
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '-')
        })
    {
        return Err(AgentPlanError::InvalidSessionSelector);
    }
    Ok(())
}

fn probe_plan(provider: AgentProvider) -> AgentCommandPlan {
    let executable = provider.executable();
    let mut commands = vec![
        capture(
            format!("command -v {executable} 2>/dev/null || true"),
            "Locate the CLI without modifying the host",
            5,
        ),
        capture(
            format!("{executable} --version 2>/dev/null || true"),
            "Read the installed version",
            10,
        ),
        capture(
            format!("{executable} --help 2>/dev/null || true"),
            "Discover capabilities advertised by this installed version",
            10,
        ),
    ];
    match provider {
        AgentProvider::Codex => commands.push(capture(
            "codex login status 2>/dev/null || true".to_owned(),
            "Read Codex login status through the CLI only",
            10,
        )),
        AgentProvider::OpenCode => commands.push(capture(
            "opencode auth list 2>/dev/null || true".to_owned(),
            "List configured OpenCode providers through the CLI only",
            10,
        )),
        AgentProvider::ClaudeCode | AgentProvider::GeminiCli => {}
    }
    base_plan(
        provider,
        AgentAction::Probe,
        BasePlanOptions {
            commands,
            requires_confirmation: false,
            source_url: None,
            tmux_session: None,
            notice: "Probe output is bounded by the caller and must never include credential files.",
        },
    )
}

fn install_plan(provider: AgentProvider) -> AgentCommandPlan {
    let (command, source_url, notice) = match provider {
        AgentProvider::Codex => (
            "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            "https://chatgpt.com/codex/install.sh",
            "Downloads and executes the official Codex installer as the current remote user.",
        ),
        AgentProvider::ClaudeCode => (
            "npm install -g @anthropic-ai/claude-code",
            "https://docs.anthropic.com/en/docs/claude-code/getting-started",
            "Installs the official Claude Code npm package globally; do not prefix it with sudo.",
        ),
        AgentProvider::GeminiCli => (
            "npm install -g @google/gemini-cli",
            "https://github.com/google-gemini/gemini-cli",
            "Installs the official Gemini CLI npm package globally as the current remote user.",
        ),
        AgentProvider::OpenCode => (
            "curl -fsSL https://opencode.ai/install | bash",
            "https://opencode.ai/install",
            "Downloads and executes the official OpenCode installer as the current remote user.",
        ),
    };
    base_plan(
        provider,
        AgentAction::Install,
        BasePlanOptions {
            commands: vec![interactive(
                command.to_owned(),
                "Run the reviewed installer",
            )],
            requires_confirmation: true,
            source_url: Some(source_url),
            tmux_session: None,
            notice,
        },
    )
}

fn login_plan(provider: AgentProvider, workspace: &str) -> AgentCommandPlan {
    let command = match provider {
        AgentProvider::Codex => "exec codex login",
        // Claude Code and Gemini intentionally keep account authorization in
        // their own interactive startup flow rather than accepting a token.
        AgentProvider::ClaudeCode => "exec claude",
        AgentProvider::GeminiCli => "exec gemini",
        AgentProvider::OpenCode => "exec opencode auth login",
    };
    base_plan(
        provider,
        AgentAction::Login,
        BasePlanOptions {
            commands: vec![interactive(
                in_workspace(command, workspace),
                "Open the provider-owned interactive authentication flow",
            )],
            requires_confirmation: false,
            source_url: None,
            tmux_session: None,
            notice: "Credentials remain inside the provider CLI and its external authorization flow.",
        },
    )
}

fn update_plan(provider: AgentProvider, workspace: &str) -> AgentCommandPlan {
    let command = match provider {
        AgentProvider::Codex => "exec codex update",
        AgentProvider::ClaudeCode => "exec claude update",
        AgentProvider::GeminiCli => "exec gemini update",
        AgentProvider::OpenCode => "exec opencode upgrade",
    };
    base_plan(
        provider,
        AgentAction::Update,
        BasePlanOptions {
            commands: vec![interactive(
                in_workspace(command, workspace),
                "Run the update command advertised by the installed CLI",
            )],
            requires_confirmation: true,
            source_url: None,
            tmux_session: None,
            notice: "Review the installed version's help output and update impact before execution.",
        },
    )
}

fn start_plan(request: &AgentPlanRequest) -> AgentCommandPlan {
    provider_tmux_plan(request, request.provider.executable(), AgentAction::Start)
}

fn resume_plan(request: &AgentPlanRequest) -> AgentCommandPlan {
    let invocation = match request.provider {
        AgentProvider::Codex => "codex resume --last",
        AgentProvider::ClaudeCode => "claude --continue",
        AgentProvider::GeminiCli => "gemini --resume latest",
        AgentProvider::OpenCode => "opencode --continue",
    };
    provider_tmux_plan(request, invocation, AgentAction::Resume)
}

fn provider_tmux_plan(
    request: &AgentPlanRequest,
    new_session_invocation: &str,
    action: AgentAction,
) -> AgentCommandPlan {
    let (session, command, purpose, notice) = if let Some(selector) =
        request.resume_session.as_deref()
    {
        (
            selector.to_owned(),
            format!("exec tmux attach-session -t {}", shell_quote(selector)),
            "Attach to the selected RemoteDeck-owned tmux session",
            "Attached to the selected provider-specific RemoteDeck tmux session.",
        )
    } else {
        let session =
            stable_agent_tmux_session(request.provider, &request.host_id, &request.workspace)
                .expect("request was validated before planning");
        let command = format!(
            "exec tmux new-session -A -s {} -c {} {}",
            shell_quote(&session),
            shell_path(&request.workspace),
            shell_quote(new_session_invocation)
        );
        (
            session,
            command,
            "Attach to or create the stable provider-specific RemoteDeck tmux session",
            "The provider runs with its normal permission model inside a stable RemoteDeck tmux session.",
        )
    };
    base_plan(
        request.provider,
        action,
        BasePlanOptions {
            commands: vec![interactive(command, purpose)],
            requires_confirmation: false,
            source_url: None,
            tmux_session: Some(session.as_str()),
            notice,
        },
    )
}

fn in_workspace(command: &str, workspace: &str) -> String {
    format!("cd -- {} && {command}", shell_path(workspace))
}

fn shell_path(path: &str) -> String {
    if path == "~" {
        "\"$HOME\"".to_owned()
    } else if let Some(remainder) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", shell_quote(remainder))
    } else {
        shell_quote(path)
    }
}

fn capture(command: String, purpose: &str, timeout_seconds: u64) -> PlannedCommand {
    PlannedCommand {
        command,
        mode: PlanMode::Capture,
        purpose: purpose.to_owned(),
        timeout_seconds: Some(timeout_seconds),
    }
}

fn interactive(command: String, purpose: &str) -> PlannedCommand {
    PlannedCommand {
        command,
        mode: PlanMode::InteractivePty,
        purpose: purpose.to_owned(),
        timeout_seconds: None,
    }
}

struct BasePlanOptions<'a> {
    commands: Vec<PlannedCommand>,
    requires_confirmation: bool,
    source_url: Option<&'a str>,
    tmux_session: Option<&'a str>,
    notice: &'a str,
}

fn base_plan(
    provider: AgentProvider,
    action: AgentAction,
    options: BasePlanOptions<'_>,
) -> AgentCommandPlan {
    AgentCommandPlan {
        provider,
        action,
        display_name: provider.display_name().to_owned(),
        commands: options.commands,
        requires_confirmation: options.requires_confirmation,
        source_url: options.source_url.map(ToOwned::to_owned),
        tmux_session: options.tmux_session.map(ToOwned::to_owned),
        notice: options.notice.to_owned(),
    }
}

fn ensure_plan_is_safe(plan: &AgentCommandPlan) -> Result<(), AgentPlanError> {
    const FORBIDDEN: &[&str] = &[
        "--dangerously-bypass-approvals-and-sandbox",
        "--dangerously-skip-permissions",
        "--yolo",
        "--auto",
        "--auto-approve",
    ];
    if plan.commands.iter().any(|command| {
        let lower = command.command.to_ascii_lowercase();
        FORBIDDEN.iter().any(|option| lower.contains(option))
    }) {
        return Err(AgentPlanError::UnsafeGeneratedPlan);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_A: &str = "11111111-2222-4333-8444-555555555555";

    fn request(provider: AgentProvider, action: AgentAction) -> AgentPlanRequest {
        AgentPlanRequest {
            provider,
            action,
            host_id: HOST_A.to_owned(),
            workspace: "/data/项目 path/it's".to_owned(),
            resume_session: None,
        }
    }

    #[test]
    fn every_provider_has_all_six_inert_plans() {
        for provider in AgentProvider::ALL {
            for action in AgentAction::ALL {
                let plan = plan_agent_action(&request(provider, action)).expect("plan");
                assert_eq!(plan.provider, provider);
                assert_eq!(plan.action, action);
                assert!(!plan.commands.is_empty());
                assert!(
                    plan.commands
                        .iter()
                        .all(|command| !command.command.is_empty())
                );
                ensure_plan_is_safe(&plan).expect("safe");
            }
        }
    }

    #[test]
    fn install_and_update_plans_require_review() {
        for provider in AgentProvider::ALL {
            let install =
                plan_agent_action(&request(provider, AgentAction::Install)).expect("install");
            assert!(install.requires_confirmation);
            assert!(install.source_url.is_some());
            let update =
                plan_agent_action(&request(provider, AgentAction::Update)).expect("update");
            assert!(update.requires_confirmation);
        }
    }

    #[test]
    fn login_plans_never_accept_or_embed_tokens() {
        for provider in AgentProvider::ALL {
            let plan = plan_agent_action(&request(provider, AgentAction::Login)).expect("login");
            let text = plan
                .commands
                .iter()
                .map(|command| command.command.as_str())
                .collect::<Vec<_>>()
                .join("\n")
                .to_ascii_lowercase();
            assert!(!text.contains("token="));
            assert!(!text.contains("api_key"));
            assert_eq!(plan.commands[0].mode, PlanMode::InteractivePty);
        }
    }

    #[test]
    fn every_provider_start_and_resume_share_a_stable_owned_tmux_session() {
        for provider in AgentProvider::ALL {
            let start = plan_agent_action(&request(provider, AgentAction::Start)).expect("start");
            let resume =
                plan_agent_action(&request(provider, AgentAction::Resume)).expect("resume");
            assert_eq!(start.tmux_session, resume.tmux_session);
            assert!(start.commands[0].command.contains("tmux new-session -A"));
            assert!(resume.commands[0].command.contains("tmux new-session -A"));
            assert!(
                start
                    .tmux_session
                    .as_deref()
                    .is_some_and(|value| is_owned_tmux_session(provider, value))
            );
        }
    }

    #[test]
    fn stable_tmux_name_changes_with_provider_or_identity_but_not_call_order() {
        let first =
            stable_agent_tmux_session(AgentProvider::Codex, HOST_A, "/work/a").expect("first");
        let repeated =
            stable_agent_tmux_session(AgentProvider::Codex, HOST_A, "/work/a").expect("repeat");
        let different =
            stable_agent_tmux_session(AgentProvider::Codex, HOST_A, "/work/b").expect("different");
        let provider = stable_agent_tmux_session(AgentProvider::ClaudeCode, HOST_A, "/work/a")
            .expect("provider");
        assert_eq!(first, repeated);
        assert_ne!(first, different);
        assert_ne!(first, provider);
        assert!(first.starts_with("remotedeck-codex-"));
        assert!(provider.starts_with("remotedeck-claude-"));
    }

    #[test]
    fn workspaces_and_owned_tmux_selectors_are_posix_quoted() {
        let plan = plan_agent_action(&request(AgentProvider::ClaudeCode, AgentAction::Start))
            .expect("start");
        assert!(
            plan.commands[0]
                .command
                .contains("'/data/项目 path/it'\\''s'")
        );
        let selected = "remotedeck-claude-session-42";
        let resume = plan_agent_action(&AgentPlanRequest {
            resume_session: Some(selected.to_owned()),
            ..request(AgentProvider::ClaudeCode, AgentAction::Resume)
        })
        .expect("resume");
        assert!(resume.commands[0].command.contains("tmux attach-session"));
        assert!(resume.commands[0].command.contains(&shell_quote(selected)));
        assert!(!resume.commands[0].command.contains("claude --continue"));
    }

    #[test]
    fn provider_resume_commands_are_used_only_when_creating_the_stable_session() {
        for (provider, invocation) in [
            (AgentProvider::Codex, "codex resume --last"),
            (AgentProvider::ClaudeCode, "claude --continue"),
            (AgentProvider::GeminiCli, "gemini --resume latest"),
            (AgentProvider::OpenCode, "opencode --continue"),
        ] {
            let plan =
                plan_agent_action(&request(provider, AgentAction::Resume)).expect("resume plan");
            assert!(plan.commands[0].command.contains(invocation));
        }

        let selected = "remotedeck-codex-existing";
        let plan = plan_agent_action(&AgentPlanRequest {
            resume_session: Some(selected.to_owned()),
            ..request(AgentProvider::Codex, AgentAction::Resume)
        })
        .expect("selected resume");
        assert!(plan.commands[0].command.contains("tmux attach-session"));
        assert!(!plan.commands[0].command.contains("codex resume"));
    }

    #[test]
    fn unsafe_inputs_fail_before_a_plan_is_returned() {
        let mut invalid = request(AgentProvider::Codex, AgentAction::Start);
        invalid.host_id = "not-a-uuid".to_owned();
        assert_eq!(
            plan_agent_action(&invalid),
            Err(AgentPlanError::InvalidHostId)
        );
        invalid = request(AgentProvider::Codex, AgentAction::Start);
        invalid.workspace = "/tmp/good\nrm -rf /".to_owned();
        assert_eq!(
            plan_agent_action(&invalid),
            Err(AgentPlanError::InvalidWorkspace)
        );
        invalid = request(AgentProvider::OpenCode, AgentAction::Resume);
        invalid.resume_session = Some("latest; reboot".to_owned());
        assert_eq!(
            plan_agent_action(&invalid),
            Err(AgentPlanError::InvalidSessionSelector)
        );
        invalid = request(AgentProvider::GeminiCli, AgentAction::Start);
        invalid.resume_session = Some("latest".to_owned());
        assert_eq!(
            plan_agent_action(&invalid),
            Err(AgentPlanError::UnexpectedSessionSelector)
        );
        invalid = request(AgentProvider::Codex, AgentAction::Resume);
        invalid.resume_session = Some("remotedeck-claude-existing".to_owned());
        assert_eq!(
            plan_agent_action(&invalid),
            Err(AgentPlanError::UnownedSessionSelector)
        );
    }

    #[test]
    fn generated_commands_never_disable_provider_safety() {
        for provider in AgentProvider::ALL {
            for action in AgentAction::ALL {
                let plan = plan_agent_action(&request(provider, action)).expect("plan");
                let all = plan
                    .commands
                    .iter()
                    .map(|command| command.command.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_ascii_lowercase();
                for forbidden in ["dangerously", "bypass", "--yolo", "--auto-approve"] {
                    assert!(!all.contains(forbidden), "{provider:?} {action:?}: {all}");
                }
            }
        }
    }
}
