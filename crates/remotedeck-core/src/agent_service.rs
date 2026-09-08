use crate::{
    agent::{
        AgentAction, AgentCommandPlan, AgentPlanRequest, AgentProvider, is_owned_tmux_session,
        plan_agent_action,
    },
    error::{AppError, AppResult},
    model::{HostProfile, TerminalSnapshot},
    ssh::SshRuntime,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const AGENT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub host_id: String,
    pub agent: AgentProvider,
    pub installed: bool,
    pub version: Option<String>,
    pub authenticated: Option<bool>,
    pub tmux_available: bool,
    pub resumable_sessions: Vec<String>,
    pub install_hint: Option<String>,
    pub documentation_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSessionRequest {
    pub host_id: String,
    pub agent: AgentProvider,
    pub action: AgentAction,
    pub workspace: Option<String>,
    pub session_name: Option<String>,
    #[serde(default)]
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionResult {
    pub terminal: TerminalSnapshot,
    pub message: String,
}

pub async fn probe_agent(
    runtime: &SshRuntime,
    host: &HostProfile,
    provider: AgentProvider,
) -> AppResult<AgentStatus> {
    let command = probe_command(provider);
    let result = runtime
        .run_command_with_timeout(host, command, None, AGENT_PROBE_TIMEOUT)
        .await?;
    let sections = parse_sections(&result.stdout);
    let binary = section(&sections, "BIN");
    let installed = !binary.trim().is_empty();
    let version = installed
        .then(|| first_nonempty(section(&sections, "VERSION")))
        .flatten();
    let auth_text = section(&sections, "AUTH").to_ascii_lowercase();
    let authenticated = authentication_state(provider, installed, &auth_text);
    let tmux_available = !section(&sections, "TMUX").trim().is_empty();
    let resumable_sessions = owned_tmux_sessions(provider, section(&sections, "SESSIONS"));
    let (install_hint, documentation_url) = provider_help(provider);
    Ok(AgentStatus {
        host_id: host.id.clone(),
        agent: provider,
        installed,
        version,
        authenticated,
        tmux_available,
        resumable_sessions,
        install_hint: (!installed).then(|| install_hint.to_owned()),
        documentation_url: Some(documentation_url.to_owned()),
    })
}

fn probe_command(provider: AgentProvider) -> String {
    let executable = provider.executable();
    let login_command = match provider {
        AgentProvider::Codex => "codex login status",
        AgentProvider::ClaudeCode => "claude auth status",
        AgentProvider::GeminiCli => "gemini --version",
        AgentProvider::OpenCode => "opencode auth list",
    };
    let session_prefix = provider.tmux_session_prefix();
    format!(
        "printf '__RD_BIN__\\n'; command -v {executable} 2>/dev/null || true; \
         printf '__RD_VERSION__\\n'; {executable} --version 2>&1 || true; \
         printf '__RD_AUTH__\\n'; {login_command} 2>&1 || true; \
         printf '__RD_TMUX__\\n'; command -v tmux 2>/dev/null || true; \
         printf '__RD_SESSIONS__\\n'; tmux list-sessions -F '#S' 2>/dev/null | \
         while IFS= read -r rd_session; do case \"$rd_session\" in {session_prefix}*) \
         printf '%s\\n' \"$rd_session\";; esac; done | head -n 200 || true"
    )
}

pub fn session_plan(
    host: &HostProfile,
    request: &AgentSessionRequest,
) -> AppResult<AgentCommandPlan> {
    if request.host_id != host.id {
        return Err(AppError::Validation(
            "agent request host does not match the selected profile".to_owned(),
        ));
    }
    let workspace = request
        .workspace
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(host.default_workspace.as_str())
        .to_owned();
    let resume_session = request
        .session_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let plan = plan_agent_action(&AgentPlanRequest {
        provider: request.agent,
        action: request.action,
        host_id: host.id.clone(),
        workspace,
        resume_session,
    })
    .map_err(|error| AppError::Validation(error.to_string()))?;
    Ok(plan)
}

pub fn confirmed_session_plan(
    host: &HostProfile,
    request: &AgentSessionRequest,
) -> AppResult<AgentCommandPlan> {
    let plan = session_plan(host, request)?;
    if plan.requires_confirmation {
        if request.confirmation.as_deref() != Some(host.alias.as_str()) {
            return Err(AppError::Validation(format!(
                "type host alias '{}' exactly to confirm this agent action",
                host.alias
            )));
        }
    } else if request.confirmation.is_some() {
        return Err(AppError::Validation(
            "confirmation is only accepted for agent actions that require it".to_owned(),
        ));
    }
    Ok(plan)
}

fn owned_tmux_sessions(provider: AgentProvider, value: &str) -> Vec<String> {
    let mut sessions = value
        .lines()
        .map(str::trim)
        .filter(|session| is_owned_tmux_session(provider, session))
        .take(200)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    sessions.sort();
    sessions.dedup();
    sessions
}

fn authentication_state(provider: AgentProvider, installed: bool, output: &str) -> Option<bool> {
    if !installed {
        return None;
    }
    if output.contains("not logged")
        || output.contains("logged out")
        || output.contains("not authenticated")
        || output.contains("no auth")
    {
        return Some(false);
    }
    if output.contains("logged in")
        || output.contains("authenticated")
        || output.contains("chatgpt")
        || output.contains("api key")
    {
        return Some(true);
    }
    match provider {
        AgentProvider::OpenCode if !output.trim().is_empty() => Some(true),
        AgentProvider::Codex
        | AgentProvider::ClaudeCode
        | AgentProvider::GeminiCli
        | AgentProvider::OpenCode => None,
    }
}

fn provider_help(provider: AgentProvider) -> (&'static str, &'static str) {
    match provider {
        AgentProvider::Codex => (
            "Use the reviewed official Codex install action.",
            "https://developers.openai.com/codex/cli/",
        ),
        AgentProvider::ClaudeCode => (
            "Install @anthropic-ai/claude-code as the current remote user.",
            "https://docs.anthropic.com/en/docs/claude-code/getting-started",
        ),
        AgentProvider::GeminiCli => (
            "Install @google/gemini-cli as the current remote user.",
            "https://github.com/google-gemini/gemini-cli",
        ),
        AgentProvider::OpenCode => (
            "Use the reviewed official OpenCode install action.",
            "https://opencode.ai/docs/",
        ),
    }
}

fn parse_sections(output: &str) -> std::collections::HashMap<String, String> {
    let mut sections = std::collections::HashMap::new();
    let mut current: Option<String> = None;
    for line in output.lines() {
        if let Some(name) = line
            .strip_prefix("__RD_")
            .and_then(|value| value.strip_suffix("__"))
            .filter(|value| !value.is_empty())
        {
            current = Some(name.to_owned());
            sections.entry(name.to_owned()).or_insert_with(String::new);
        } else if let Some(name) = current.as_ref() {
            let target = sections.entry(name.clone()).or_insert_with(String::new);
            if !target.is_empty() {
                target.push('\n');
            }
            target.push_str(line);
        }
    }
    sections
}

fn section<'a>(sections: &'a std::collections::HashMap<String, String>, name: &str) -> &'a str {
    sections.get(name).map(String::as_str).unwrap_or_default()
}

fn first_nonempty(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(512).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, SshAdvancedOptions};
    use chrono::Utc;

    const HOST_ID: &str = "11111111-2222-4333-8444-555555555555";

    fn host() -> HostProfile {
        let now = Utc::now();
        HostProfile {
            schema_version: 2,
            id: HOST_ID.to_owned(),
            alias: "Lab-A".to_owned(),
            hostname: "lab.example.test".to_owned(),
            port: 22,
            username: "alice".to_owned(),
            auth_method: AuthMethod::Interactive,
            identity_file: None,
            proxy_jump: None,
            default_workspace: "/srv/project".to_owned(),
            groups: Vec::new(),
            advanced: SshAdvancedOptions::default(),
            monitor_enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn request(action: AgentAction) -> AgentSessionRequest {
        AgentSessionRequest {
            host_id: HOST_ID.to_owned(),
            agent: AgentProvider::Codex,
            action,
            workspace: None,
            session_name: None,
            confirmation: None,
        }
    }

    #[test]
    fn provider_names_match_the_frontend_contract() {
        assert_eq!(
            serde_json::to_string(&AgentProvider::Codex).unwrap(),
            "\"codex\""
        );
        assert_eq!(
            serde_json::to_string(&AgentProvider::ClaudeCode).unwrap(),
            "\"claude\""
        );
        assert_eq!(
            serde_json::to_string(&AgentProvider::GeminiCli).unwrap(),
            "\"gemini\""
        );
        assert_eq!(
            serde_json::to_string(&AgentProvider::OpenCode).unwrap(),
            "\"opencode\""
        );
    }

    #[test]
    fn parses_bounded_probe_sections() {
        let sections = parse_sections(
            "__RD_BIN__\n/usr/bin/codex\n__RD_VERSION__\ncodex 1.2.3\n__RD_AUTH__\nLogged in using ChatGPT\n__RD_TMUX__\n/usr/bin/tmux\n__RD_SESSIONS__\nremotedeck-codex-a\n",
        );
        assert_eq!(section(&sections, "BIN"), "/usr/bin/codex");
        assert_eq!(
            first_nonempty(section(&sections, "VERSION")).as_deref(),
            Some("codex 1.2.3")
        );
        assert_eq!(
            authentication_state(
                AgentProvider::Codex,
                true,
                &section(&sections, "AUTH").to_ascii_lowercase()
            ),
            Some(true)
        );
    }

    #[test]
    fn negative_authentication_is_not_misread_as_authenticated() {
        assert_eq!(
            authentication_state(AgentProvider::Codex, true, "not logged in"),
            Some(false)
        );
    }

    #[test]
    fn install_and_update_require_the_exact_host_alias() {
        let host = host();
        for action in [AgentAction::Install, AgentAction::Update] {
            let missing = request(action);
            assert!(
                confirmed_session_plan(&host, &missing)
                    .expect_err("missing confirmation")
                    .to_string()
                    .contains("Lab-A")
            );

            let mut wrong_case = request(action);
            wrong_case.confirmation = Some("lab-a".to_owned());
            assert!(confirmed_session_plan(&host, &wrong_case).is_err());

            let mut exact = request(action);
            exact.confirmation = Some(host.alias.clone());
            assert!(
                confirmed_session_plan(&host, &exact)
                    .expect("confirmed")
                    .requires_confirmation
            );
        }
    }

    #[test]
    fn non_confirming_actions_reject_confirmation_payloads() {
        let host = host();
        let mut unexpected = request(AgentAction::Login);
        unexpected.confirmation = Some(host.alias.clone());
        assert!(
            confirmed_session_plan(&host, &unexpected)
                .expect_err("unexpected confirmation")
                .to_string()
                .contains("only accepted")
        );
        confirmed_session_plan(&host, &request(AgentAction::Login)).expect("null confirmation");
    }

    #[test]
    fn plan_preview_does_not_consume_execution_confirmation() {
        let host = host();
        let plan = session_plan(&host, &request(AgentAction::Install)).expect("preview");
        assert!(plan.requires_confirmation);
    }

    #[test]
    fn resumable_sessions_include_only_the_requested_provider_owned_prefix() {
        let sessions = owned_tmux_sessions(
            AgentProvider::Codex,
            "personal\nremotedeck-claude-a\nremotedeck-codex-z\nremotedeck-codex-a\nremotedeck-codex-a\nremotedeck-codex-\nremotedeck-codex-bad/name\n",
        );
        assert_eq!(
            sessions,
            vec![
                "remotedeck-codex-a".to_owned(),
                "remotedeck-codex-z".to_owned()
            ]
        );
        let command = probe_command(AgentProvider::GeminiCli);
        assert!(command.contains("remotedeck-gemini-*)"));
        assert!(!command.contains("remotedeck-codex-*)"));
    }

    #[test]
    fn confirmation_is_optional_in_the_frontend_contract() {
        let parsed: AgentSessionRequest = serde_json::from_value(serde_json::json!({
            "hostId": HOST_ID,
            "agent": "codex",
            "action": "start",
            "workspace": null,
            "sessionName": null
        }))
        .expect("request without confirmation");
        assert!(parsed.confirmation.is_none());
    }
}
