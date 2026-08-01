//! Pure command-risk policy for saved presets.
//!
//! This module deliberately does not execute commands.  Callers must reload the
//! final command text, run [`analyze_command`], and enforce the returned
//! confirmation immediately before opening an SSH exec channel or PTY.

use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

const MAX_COMMAND_BYTES: usize = 32 * 1024;
const MAX_CONFIRMATION_CHARS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RiskLevel {
    L0,
    L1,
    L2,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfirmationRequirement {
    None,
    Acknowledge,
    TypeExact { value: String },
}

impl ConfirmationRequirement {
    /// Checks UI confirmation without weakening the policy decision.
    #[cfg(test)]
    pub fn is_satisfied(&self, acknowledged: bool, typed: Option<&str>) -> bool {
        match self {
            Self::None => true,
            Self::Acknowledge => acknowledged,
            Self::TypeExact { value } => acknowledged && typed == Some(value.as_str()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RiskAnalysis {
    pub declared_risk: RiskLevel,
    pub detected_risk: RiskLevel,
    pub effective_risk: RiskLevel,
    pub reasons: Vec<String>,
    pub confirmation: ConfirmationRequirement,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskError {
    InvalidCommand,
    InvalidConfirmationText,
    MissingL2ConfirmationText,
}

impl fmt::Display for RiskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCommand => {
                formatter.write_str("command is empty, oversized, or contains NUL")
            }
            Self::InvalidConfirmationText => {
                formatter.write_str("confirmation text is oversized or contains control characters")
            }
            Self::MissingL2ConfirmationText => {
                formatter.write_str("L2 commands require non-empty exact confirmation text")
            }
        }
    }
}

impl Error for RiskError {}

/// An immutable preset shipped by the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub command: &'static str,
    pub declared_risk: RiskLevel,
    pub requires_pty: bool,
    pub requires_sudo: bool,
    pub dependency: Option<&'static str>,
}

pub const BUILTIN_PRESETS: &[BuiltinPreset] = &[
    BuiltinPreset {
        id: "system-overview",
        name: "System overview",
        group: "System",
        command: "uname -a; hostname; whoami; uptime; lscpu | head -n 20",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: None,
    },
    BuiltinPreset {
        id: "gpu-status",
        name: "GPU status",
        group: "Resources",
        command: "nvidia-smi",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: Some("nvidia-smi"),
    },
    BuiltinPreset {
        id: "cpu-memory-processes",
        name: "CPU and memory processes",
        group: "Resources",
        command: "ps -eo pid,ppid,user,pcpu,pmem,state,etime,args --sort=-pcpu | head -n 31",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: None,
    },
    BuiltinPreset {
        id: "disk-and-inodes",
        name: "Disk and inode usage",
        group: "Resources",
        command: "df -hT; printf '\\n'; df -ih",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: None,
    },
    BuiltinPreset {
        id: "current-user-processes",
        name: "Current-user processes",
        group: "System",
        command: "ps -u \"$USER\" -o pid,ppid,pcpu,pmem,state,etime,args --sort=-pcpu",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: None,
    },
    BuiltinPreset {
        id: "listeners",
        name: "Listening sockets",
        group: "Network",
        command: "ss -lntup",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: Some("ss"),
    },
    BuiltinPreset {
        id: "git-status",
        name: "Git status",
        group: "Development",
        command: "git status --short --branch",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: Some("git"),
    },
    BuiltinPreset {
        id: "slurm-queue",
        name: "Slurm queue",
        group: "Scheduler",
        command: "squeue -u \"$USER\"",
        declared_risk: RiskLevel::L0,
        requires_pty: false,
        requires_sudo: false,
        dependency: Some("squeue"),
    },
    BuiltinPreset {
        id: "btop",
        name: "btop",
        group: "Resources",
        command: "exec btop",
        declared_risk: RiskLevel::L0,
        requires_pty: true,
        requires_sudo: false,
        dependency: Some("btop"),
    },
    BuiltinPreset {
        id: "reboot-workstation",
        name: "Reboot workstation",
        group: "High risk",
        command: "systemctl reboot",
        declared_risk: RiskLevel::L2,
        requires_pty: true,
        requires_sudo: true,
        dependency: Some("systemctl"),
    },
];

pub fn builtin_presets() -> &'static [BuiltinPreset] {
    BUILTIN_PRESETS
}

pub fn builtin_preset(id: &str) -> Option<&'static BuiltinPreset> {
    BUILTIN_PRESETS.iter().find(|preset| preset.id == id)
}

/// Conservatively classifies a complete, final shell command.
///
/// Unknown commands never fall below L1.  Privilege elevation, process
/// signalling, deletion, reboot, raw-disk operations, remote-script execution,
/// and other destructive constructs are L2.
pub fn analyze_command(
    command: &str,
    declared_risk: RiskLevel,
    l2_confirmation_text: Option<&str>,
) -> Result<RiskAnalysis, RiskError> {
    validate_command(command)?;
    let lower = command.to_ascii_lowercase();
    let mut reasons = Vec::new();
    let mut detected_risk = if is_read_only_command(command) {
        RiskLevel::L0
    } else {
        push_reason(
            &mut reasons,
            "Command is not wholly covered by the read-only allowlist",
        );
        RiskLevel::L1
    };

    for (matched, reason) in level_one_findings(&lower) {
        if matched {
            detected_risk = detected_risk.max(RiskLevel::L1);
            push_reason(&mut reasons, reason);
        }
    }
    for (matched, reason) in level_two_findings(&lower) {
        if matched {
            detected_risk = RiskLevel::L2;
            push_reason(&mut reasons, reason);
        }
    }

    if declared_risk > detected_risk {
        push_reason(
            &mut reasons,
            &format!("Preset declares risk {declared_risk:?}"),
        );
    }
    let effective_risk = declared_risk.max(detected_risk);
    let confirmation = match effective_risk {
        RiskLevel::L0 => ConfirmationRequirement::None,
        RiskLevel::L1 => ConfirmationRequirement::Acknowledge,
        RiskLevel::L2 => {
            let value = l2_confirmation_text
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or(RiskError::MissingL2ConfirmationText)?;
            validate_confirmation_text(value)?;
            ConfirmationRequirement::TypeExact {
                value: value.to_owned(),
            }
        }
    };

    Ok(RiskAnalysis {
        declared_risk,
        detected_risk,
        effective_risk,
        reasons,
        confirmation,
    })
}

fn validate_command(command: &str) -> Result<(), RiskError> {
    if command.trim().is_empty()
        || command.len() > MAX_COMMAND_BYTES
        || command.as_bytes().contains(&0)
    {
        return Err(RiskError::InvalidCommand);
    }
    Ok(())
}

fn validate_confirmation_text(value: &str) -> Result<(), RiskError> {
    if value.chars().count() > MAX_CONFIRMATION_CHARS || value.chars().any(char::is_control) {
        return Err(RiskError::InvalidConfirmationText);
    }
    Ok(())
}

fn level_one_findings(command: &str) -> Vec<(bool, &'static str)> {
    vec![
        (
            contains_any_command(command, &["cp", "mv", "mkdir", "touch", "chmod", "chown"]),
            "May modify files or permissions",
        ),
        (contains_service_mutation(command), "Changes service state"),
        (
            contains_package_mutation(command),
            "Changes installed software",
        ),
        (contains_mutating_git(command), "Changes Git state"),
        (contains_redirection(command), "Contains output redirection"),
        (
            contains_any_command(command, &["tee", "truncate"]) || contains_sed_in_place(command),
            "Uses a file-writing utility",
        ),
        (
            command.contains("$(") || command.contains('`'),
            "Contains command substitution",
        ),
    ]
}

fn level_two_findings(command: &str) -> Vec<(bool, &'static str)> {
    vec![
        (
            contains_command(command, "sudo") || contains_command(command, "su"),
            "Requests privilege elevation",
        ),
        (
            contains_any_command(command, &["reboot", "shutdown", "poweroff", "halt"])
                || contains_systemctl_reboot(command),
            "Can shut down or reboot the host",
        ),
        (
            contains_any_command(command, &["rm", "rmdir", "unlink"])
                || contains_find_delete(command),
            "Deletes filesystem entries",
        ),
        (
            contains_any_command(command, &["kill", "pkill", "killall"])
                || contains_systemctl_kill(command),
            "Signals or terminates processes",
        ),
        (
            contains_any_command(command, &["mkfs", "fdisk", "parted", "wipefs"])
                || contains_program_prefix(command, "mkfs.")
                || contains_dd_device_copy(command),
            "Can modify disks or filesystems",
        ),
        (
            contains_destructive_git(command),
            "Contains a destructive Git operation",
        ),
        (
            contains_remote_script_pipe(command),
            "Downloads and executes a remote script",
        ),
        (
            contains_package_removal(command),
            "Removes installed software",
        ),
    ]
}

fn is_read_only_command(command: &str) -> bool {
    let segments = split_shell_segments(command);
    !segments.is_empty() && segments.iter().all(|segment| is_read_only_segment(segment))
}

fn split_shell_segments(command: &str) -> Vec<&str> {
    command
        .split(['\n', ';', '|', '&'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn is_read_only_segment(segment: &str) -> bool {
    if segment.contains('>') || segment.contains("$(") || segment.contains('`') {
        return false;
    }
    let words = shell_words(segment);
    if words.is_empty() {
        return false;
    }
    let mut index = 0;
    while words
        .get(index)
        .is_some_and(|word| is_environment_assignment(word))
    {
        index += 1;
    }
    while words
        .get(index)
        .is_some_and(|word| matches!(word.as_str(), "command" | "exec" | "if" | "then"))
    {
        index += 1;
    }
    let Some(program) = words.get(index).map(|value| executable_basename(value)) else {
        return false;
    };
    const READ_ONLY: &[&str] = &[
        "awk",
        "btop",
        "cat",
        "cut",
        "date",
        "df",
        "du",
        "echo",
        "env",
        "find",
        "free",
        "git",
        "grep",
        "head",
        "hostname",
        "id",
        "ip",
        "journalctl",
        "ls",
        "lscpu",
        "lsblk",
        "nvidia-smi",
        "printf",
        "ps",
        "pwd",
        "sed",
        "squeue",
        "ss",
        "stat",
        "tail",
        "top",
        "uname",
        "uptime",
        "vmstat",
        "wc",
        "who",
        "whoami",
    ];
    if !READ_ONLY.contains(&program) {
        return false;
    }
    if program == "git" {
        let Some(subcommand) = words.get(index + 1).map(String::as_str) else {
            return false;
        };
        return matches!(
            subcommand,
            "status" | "branch" | "log" | "diff" | "show" | "rev-parse"
        );
    }
    if program == "find" && words.iter().any(|word| word == "-delete") {
        return false;
    }
    if program == "sed"
        && words
            .iter()
            .any(|word| word == "-i" || word.starts_with("-i"))
    {
        return false;
    }
    true
}

fn shell_words(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| {
                matches!(character, '\'' | '"' | '(' | ')' | '{' | '}' | ',')
            })
            .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

fn is_environment_assignment(value: &str) -> bool {
    let Some((name, _)) = value.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphanumeric()
                    && (index > 0 || character.is_ascii_alphabetic())
        })
}

fn executable_basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn contains_any_command(command: &str, programs: &[&str]) -> bool {
    programs
        .iter()
        .any(|program| contains_command(command, program))
}

fn contains_program_prefix(command: &str, prefix: &str) -> bool {
    shell_words(command)
        .iter()
        .map(|word| executable_basename(word))
        .any(|program| program.starts_with(prefix))
}

fn contains_command(command: &str, program: &str) -> bool {
    let bytes = command.as_bytes();
    let needle = program.as_bytes();
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    for index in 0..=bytes.len() - needle.len() {
        if &bytes[index..index + needle.len()] != needle {
            continue;
        }
        let before = index
            .checked_sub(1)
            .and_then(|position| bytes.get(position))
            .copied();
        let after = bytes.get(index + needle.len()).copied();
        let boundary_before = before.is_none_or(|value| !is_word_byte(value) || value == b'/');
        let boundary_after = after.is_none_or(|value| !is_word_byte(value));
        if boundary_before && boundary_after {
            return true;
        }
    }
    false
}

fn is_word_byte(value: u8) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-' | b'.')
}

fn contains_redirection(command: &str) -> bool {
    command.as_bytes().contains(&b'>')
}

fn contains_sed_in_place(command: &str) -> bool {
    contains_command(command, "sed")
        && shell_words(command)
            .iter()
            .any(|word| word == "-i" || word.starts_with("-i"))
}

fn contains_service_mutation(command: &str) -> bool {
    (contains_command(command, "systemctl") || contains_command(command, "service"))
        && contains_any_word(
            command,
            &["start", "stop", "restart", "reload", "enable", "disable"],
        )
}

fn contains_systemctl_reboot(command: &str) -> bool {
    contains_command(command, "systemctl")
        && contains_any_word(command, &["reboot", "poweroff", "halt"])
}

fn contains_systemctl_kill(command: &str) -> bool {
    contains_command(command, "systemctl") && contains_any_word(command, &["kill"])
}

fn contains_package_mutation(command: &str) -> bool {
    contains_any_command(
        command,
        &[
            "apt", "apt-get", "dnf", "yum", "pacman", "zypper", "snap", "npm", "pnpm", "pip",
            "pip3",
        ],
    ) && contains_any_word(command, &["install", "upgrade", "update", "add"])
}

fn contains_package_removal(command: &str) -> bool {
    contains_any_command(
        command,
        &[
            "apt", "apt-get", "dnf", "yum", "pacman", "zypper", "snap", "npm", "pnpm", "pip",
            "pip3",
        ],
    ) && contains_any_word(command, &["remove", "uninstall", "purge"])
}

fn contains_mutating_git(command: &str) -> bool {
    contains_command(command, "git")
        && contains_any_word(
            command,
            &[
                "add", "commit", "checkout", "switch", "merge", "rebase", "pull", "push",
                "restore", "tag",
            ],
        )
}

fn contains_destructive_git(command: &str) -> bool {
    if !contains_command(command, "git") {
        return false;
    }
    let words = shell_words(command);
    (words.iter().any(|word| word == "reset") && words.iter().any(|word| word == "--hard"))
        || (words.iter().any(|word| word == "clean")
            && words
                .iter()
                .any(|word| word.starts_with('-') && word.contains('f')))
        || (words.iter().any(|word| word == "push")
            && words.iter().any(|word| {
                matches!(word.as_str(), "--force" | "--force-with-lease") || word == "-f"
            }))
}

fn contains_dd_device_copy(command: &str) -> bool {
    contains_command(command, "dd") && (command.contains("if=") || command.contains("of="))
}

fn contains_find_delete(command: &str) -> bool {
    contains_command(command, "find") && contains_any_word(command, &["-delete"])
}

fn contains_remote_script_pipe(command: &str) -> bool {
    (contains_command(command, "curl") || contains_command(command, "wget"))
        && command.contains('|')
        && contains_any_command(command, &["sh", "bash", "zsh"])
}

fn contains_any_word(command: &str, words: &[&str]) -> bool {
    let tokens = shell_words(command);
    words.iter().any(|expected| {
        tokens
            .iter()
            .any(|token| token == expected || token.trim_matches('-') == expected.trim_matches('-'))
    })
}

fn push_reason(reasons: &mut Vec<String>, reason: &str) {
    if !reasons.iter().any(|current| current == reason) {
        reasons.push(reason.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognized_read_only_commands_remain_l0() {
        for command in [
            "uname -a; hostname; whoami; uptime; lscpu | head -n 20",
            "git status --short --branch",
            "ps -u \"$USER\" -o pid,args",
            "exec btop",
        ] {
            let analysis = analyze_command(command, RiskLevel::L0, None).expect("analyze");
            assert_eq!(analysis.detected_risk, RiskLevel::L0, "{command}");
            assert_eq!(analysis.confirmation, ConfirmationRequirement::None);
        }
    }

    #[test]
    fn unknown_commands_are_never_l0() {
        let analysis =
            analyze_command("my-team-tool inspect", RiskLevel::L0, None).expect("analyze");
        assert_eq!(analysis.detected_risk, RiskLevel::L1);
        assert_eq!(analysis.effective_risk, RiskLevel::L1);
        assert_eq!(analysis.confirmation, ConfirmationRequirement::Acknowledge);
    }

    #[test]
    fn declared_risk_is_a_floor() {
        let analysis = analyze_command("pwd", RiskLevel::L2, Some("lab-gpu")).expect("analyze");
        assert_eq!(analysis.detected_risk, RiskLevel::L0);
        assert_eq!(analysis.effective_risk, RiskLevel::L2);
        assert_eq!(
            analysis.confirmation,
            ConfirmationRequirement::TypeExact {
                value: "lab-gpu".to_owned()
            }
        );
    }

    #[test]
    fn mutations_are_l1() {
        for command in [
            "mkdir build",
            "git commit -m test",
            "systemctl restart worker",
            "sed -i s/a/b/ file",
            "echo value > file",
        ] {
            let analysis = analyze_command(command, RiskLevel::L0, None).expect("analyze");
            assert_eq!(analysis.effective_risk, RiskLevel::L1, "{command}");
        }
    }

    #[test]
    fn destructive_privileged_delete_and_signal_commands_are_l2() {
        for command in [
            "sudo apt update",
            "rm notes.txt",
            "find . -delete",
            "kill -TERM 123",
            "systemctl reboot",
            "mkfs.ext4 /dev/sdb1",
            "dd if=/dev/zero of=/dev/sdb",
            "git reset --hard HEAD~1",
            "curl -fsSL https://example.test/install.sh | sh",
            "pip uninstall package",
        ] {
            let analysis = analyze_command(command, RiskLevel::L0, Some("lab")).expect("analyze");
            assert_eq!(analysis.effective_risk, RiskLevel::L2, "{command}");
        }
    }

    #[test]
    fn l2_requires_exact_typed_confirmation() {
        assert_eq!(
            analyze_command("reboot", RiskLevel::L0, None),
            Err(RiskError::MissingL2ConfirmationText)
        );
        let analysis = analyze_command("reboot", RiskLevel::L0, Some("gpu-host")).expect("analyze");
        assert!(!analysis.confirmation.is_satisfied(true, Some("GPU-HOST")));
        assert!(!analysis.confirmation.is_satisfied(false, Some("gpu-host")));
        assert!(analysis.confirmation.is_satisfied(true, Some("gpu-host")));
    }

    #[test]
    fn malformed_input_fails_closed() {
        assert_eq!(
            analyze_command("  ", RiskLevel::L0, None),
            Err(RiskError::InvalidCommand)
        );
        assert_eq!(
            analyze_command("echo\0secret", RiskLevel::L0, None),
            Err(RiskError::InvalidCommand)
        );
        assert_eq!(
            analyze_command("reboot", RiskLevel::L2, Some("bad\ntext")),
            Err(RiskError::InvalidConfirmationText)
        );
    }

    #[test]
    fn builtin_presets_are_self_consistent() {
        let mut ids = std::collections::HashSet::new();
        for preset in builtin_presets() {
            assert!(ids.insert(preset.id), "duplicate preset id {}", preset.id);
            let confirmation = (preset.declared_risk == RiskLevel::L2).then_some("host");
            let analysis = analyze_command(preset.command, preset.declared_risk, confirmation)
                .expect("builtin");
            assert!(analysis.effective_risk >= preset.declared_risk);
            if preset.requires_sudo {
                assert_eq!(analysis.effective_risk, RiskLevel::L2);
            }
        }
        assert!(builtin_preset("git-status").is_some());
        assert!(builtin_preset("missing").is_none());
    }
}
