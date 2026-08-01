//! Pure SSH key and OpenSSH-config utilities.
//!
//! No function in this module reads or writes a path.  File selection,
//! persistence, ACL changes, and remote deployment remain explicit native
//! operations owned by higher layers.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fmt};

const MAX_PUBLIC_KEY_BYTES: usize = 16 * 1024;
const MAX_COMMENT_CHARS: usize = 256;
const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;
const MAX_CONFIG_LINE_BYTES: usize = 16 * 1024;
const MAX_HOST_CANDIDATES: usize = 512;
const MAX_RETAINED_LINES_PER_KIND: usize = 8;
const MAX_RETAINED_BYTES_PER_KIND: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyUtilityError {
    InvalidPublicKey,
    InvalidEd25519Blob,
    InvalidBase64,
    ConfigTooLarge,
    ConfigLineTooLong,
    TooManyHostCandidates,
}

impl fmt::Display for KeyUtilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPublicKey => formatter.write_str("invalid OpenSSH public-key entry"),
            Self::InvalidEd25519Blob => {
                formatter.write_str("public key is not a valid ssh-ed25519 key blob")
            }
            Self::InvalidBase64 => formatter.write_str("invalid base64 public-key payload"),
            Self::ConfigTooLarge => formatter.write_str("OpenSSH config exceeds 4 MiB"),
            Self::ConfigLineTooLong => formatter.write_str("OpenSSH config line exceeds 16 KiB"),
            Self::TooManyHostCandidates => {
                formatter.write_str("OpenSSH config contains too many concrete hosts")
            }
        }
    }
}

impl Error for KeyUtilityError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedKey {
    pub algorithm: String,
    pub public_key_base64: String,
    pub comment: String,
    pub material: String,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizedKeyMergeResult {
    pub content: String,
    pub already_present: bool,
    pub material: String,
    pub sha256_fingerprint: String,
}

/// Parses an authorized_keys line. Comments, blank lines, and malformed lines
/// return `None`; no shell interpretation is performed.
pub fn parse_authorized_key(line: &str) -> Option<AuthorizedKey> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.len() > MAX_PUBLIC_KEY_BYTES {
        return None;
    }
    let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
    let algorithm_index = tokens.iter().position(|token| is_key_algorithm(token))?;
    let algorithm = *tokens.get(algorithm_index)?;
    let payload = *tokens.get(algorithm_index + 1)?;
    if decode_base64(payload).is_err() {
        return None;
    }
    if algorithm == "ssh-ed25519" && validate_ed25519_payload(payload).is_err() {
        return None;
    }
    let comment = clean_key_comment(&tokens[algorithm_index + 2..].join(" "));
    Some(AuthorizedKey {
        algorithm: algorithm.to_owned(),
        public_key_base64: payload.to_owned(),
        comment,
        material: format!("{algorithm} {payload}"),
    })
}

/// Returns a canonical ssh-ed25519 line and drops any authorized_keys options.
pub fn canonical_ed25519_public_key(line: &str) -> Result<String, KeyUtilityError> {
    let parsed = parse_authorized_key(line).ok_or(KeyUtilityError::InvalidPublicKey)?;
    if parsed.algorithm != "ssh-ed25519" {
        return Err(KeyUtilityError::InvalidEd25519Blob);
    }
    validate_ed25519_payload(&parsed.public_key_base64)?;
    Ok(if parsed.comment.is_empty() {
        parsed.material
    } else {
        format!("{} {}", parsed.material, parsed.comment)
    })
}

pub fn ed25519_sha256_fingerprint(line: &str) -> Result<String, KeyUtilityError> {
    let parsed = parse_authorized_key(line).ok_or(KeyUtilityError::InvalidPublicKey)?;
    if parsed.algorithm != "ssh-ed25519" {
        return Err(KeyUtilityError::InvalidEd25519Blob);
    }
    let blob = validate_ed25519_payload(&parsed.public_key_base64)?;
    Ok(format!(
        "SHA256:{}",
        encode_base64_no_padding(&Sha256::digest(blob))
    ))
}

/// Normalizes line endings and trailing whitespace without rewriting user
/// options, comments, or unknown lines.
#[cfg(test)]
pub fn normalize_authorized_keys(value: &str) -> String {
    let mut lines = value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::trim_end)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// Material-based, idempotent merge suitable for an atomic deployment layer.
#[cfg(test)]
pub fn merge_authorized_key(
    existing: &str,
    public_key: &str,
) -> Result<AuthorizedKeyMergeResult, KeyUtilityError> {
    let canonical = canonical_ed25519_public_key(public_key)?;
    let candidate = parse_authorized_key(&canonical).ok_or(KeyUtilityError::InvalidPublicKey)?;
    let fingerprint = ed25519_sha256_fingerprint(&canonical)?;
    let already_present = existing.lines().any(|line| {
        parse_authorized_key(line).is_some_and(|parsed| parsed.material == candidate.material)
    });
    let mut content = normalize_authorized_keys(existing);
    if !already_present {
        content.push_str(&canonical);
        content.push('\n');
    }
    Ok(AuthorizedKeyMergeResult {
        content,
        already_present,
        material: candidate.material,
        sha256_fingerprint: fingerprint,
    })
}

/// Removes controls, collapses whitespace, and applies a character (not byte)
/// bound so Unicode comments remain valid UTF-8.
pub fn clean_key_comment(value: &str) -> String {
    let without_controls = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    without_controls
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_COMMENT_CHARS)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSshHostCandidate {
    pub alias: String,
    pub hostname: String,
    pub username: Option<String>,
    pub port: u16,
    pub identity_file: Option<String>,
    pub identities_only: Option<bool>,
    pub proxy_jump: Option<String>,
    pub server_alive_interval_seconds: Option<u64>,
    pub server_alive_count_max: Option<u32>,
    pub tcp_keep_alive: Option<bool>,
    pub connect_timeout_seconds: Option<u64>,
    pub compression: Option<bool>,
    pub local_forwards: Vec<String>,
    pub remote_forwards: Vec<String>,
    pub unsupported_lines: Vec<String>,
    pub local_forward_count: usize,
    pub remote_forward_count: usize,
    pub unsupported_count: usize,
}

impl OpenSshHostCandidate {
    fn new(alias: String) -> Self {
        Self {
            hostname: alias.clone(),
            alias,
            username: None,
            port: 22,
            identity_file: None,
            identities_only: None,
            proxy_jump: None,
            server_alive_interval_seconds: None,
            server_alive_count_max: None,
            tcp_keep_alive: None,
            connect_timeout_seconds: None,
            compression: None,
            local_forwards: Vec::new(),
            remote_forwards: Vec::new(),
            unsupported_lines: Vec::new(),
            local_forward_count: 0,
            remote_forward_count: 0,
            unsupported_count: 0,
        }
    }
}

/// Parses concrete `Host` blocks into review candidates. Wildcards, negated
/// patterns, global options, includes, and unsupported directives are never
/// followed or executed.
pub fn parse_open_ssh_config(text: &str) -> Result<Vec<OpenSshHostCandidate>, KeyUtilityError> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(KeyUtilityError::ConfigTooLarge);
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut candidates = Vec::new();
    let mut current = Vec::<usize>::new();
    for raw_line in normalized.split('\n') {
        if raw_line.len() > MAX_CONFIG_LINE_BYTES {
            return Err(KeyUtilityError::ConfigLineTooLong);
        }
        let content = strip_comment(raw_line).trim();
        if content.is_empty() {
            continue;
        }
        let Some((keyword, value)) = split_directive(content) else {
            push_unsupported(&mut candidates, &current, raw_line);
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            current.clear();
            for alias in split_words(value) {
                if !is_concrete_alias(&alias) {
                    continue;
                }
                if candidates.len() >= MAX_HOST_CANDIDATES {
                    return Err(KeyUtilityError::TooManyHostCandidates);
                }
                candidates.push(OpenSshHostCandidate::new(alias));
                current.push(candidates.len() - 1);
            }
            continue;
        }
        for index in current.iter().copied() {
            let candidate = candidates
                .get_mut(index)
                .expect("current indexes originate from candidates");
            apply_directive(candidate, keyword, value, raw_line);
        }
    }
    Ok(candidates)
}

fn apply_directive(
    candidate: &mut OpenSshHostCandidate,
    keyword: &str,
    value: &str,
    raw_line: &str,
) {
    let key = keyword.to_ascii_lowercase();
    let value = unquote(value.trim());
    match key.as_str() {
        "hostname" if valid_config_value(&value, 512) => candidate.hostname = value,
        "user" if valid_config_value(&value, 256) => candidate.username = Some(value),
        "port" => match value.parse::<u16>().ok().filter(|port| *port > 0) {
            Some(port) => candidate.port = port,
            None => record_unsupported(candidate, raw_line),
        },
        "identityfile" if valid_config_value(&value, 32_767) => {
            if candidate.identity_file.is_none() {
                candidate.identity_file = Some(value);
            } else {
                record_unsupported(candidate, raw_line);
            }
        }
        "identitiesonly" => match parse_boolean(&value) {
            Some(parsed) => candidate.identities_only = Some(parsed),
            None => record_unsupported(candidate, raw_line),
        },
        "proxyjump" if valid_config_value(&value, 1024) => candidate.proxy_jump = Some(value),
        "serveraliveinterval" => match parse_u64(&value, 3600) {
            Some(parsed) => candidate.server_alive_interval_seconds = Some(parsed),
            None => record_unsupported(candidate, raw_line),
        },
        "serveralivecountmax" => match parse_u32(&value, 100) {
            Some(parsed) => candidate.server_alive_count_max = Some(parsed),
            None => record_unsupported(candidate, raw_line),
        },
        "tcpkeepalive" => match parse_boolean(&value) {
            Some(parsed) => candidate.tcp_keep_alive = Some(parsed),
            None => record_unsupported(candidate, raw_line),
        },
        "connecttimeout" => match parse_u64(&value, 300) {
            Some(parsed) => candidate.connect_timeout_seconds = Some(parsed),
            None => record_unsupported(candidate, raw_line),
        },
        "compression" => match parse_boolean(&value) {
            Some(parsed) => candidate.compression = Some(parsed),
            None => record_unsupported(candidate, raw_line),
        },
        "localforward" if valid_config_value(&value, 4096) => {
            candidate.local_forward_count = candidate.local_forward_count.saturating_add(1);
            retain_bounded_line(&mut candidate.local_forwards, &value);
        }
        "remoteforward" if valid_config_value(&value, 4096) => {
            candidate.remote_forward_count = candidate.remote_forward_count.saturating_add(1);
            retain_bounded_line(&mut candidate.remote_forwards, &value);
        }
        _ => record_unsupported(candidate, raw_line),
    }
}

fn parse_boolean(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

fn parse_u64(value: &str, maximum: u64) -> Option<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed <= maximum)
}

fn parse_u32(value: &str, maximum: u32) -> Option<u32> {
    value
        .parse::<u32>()
        .ok()
        .filter(|parsed| *parsed <= maximum)
}

fn push_unsupported(candidates: &mut [OpenSshHostCandidate], current: &[usize], raw_line: &str) {
    for index in current.iter().copied() {
        if let Some(candidate) = candidates.get_mut(index) {
            record_unsupported(candidate, raw_line);
        }
    }
}

fn record_unsupported(candidate: &mut OpenSshHostCandidate, raw_line: &str) {
    candidate.unsupported_count = candidate.unsupported_count.saturating_add(1);
    retain_bounded_line(&mut candidate.unsupported_lines, raw_line);
}

fn retain_bounded_line(lines: &mut Vec<String>, line: &str) {
    if lines.len() >= MAX_RETAINED_LINES_PER_KIND
        || lines
            .iter()
            .map(String::len)
            .sum::<usize>()
            .saturating_add(line.len())
            > MAX_RETAINED_BYTES_PER_KIND
    {
        return;
    }
    lines.push(line.to_owned());
}

fn is_key_algorithm(value: &str) -> bool {
    value.starts_with("ssh-")
        || value.starts_with("ecdsa-")
        || value.starts_with("sk-")
        || value.starts_with("rsa-sha2-")
}

fn validate_ed25519_payload(payload: &str) -> Result<Vec<u8>, KeyUtilityError> {
    let blob = decode_base64(payload)?;
    let mut cursor = 0;
    let algorithm = read_ssh_string(&blob, &mut cursor)?;
    let key = read_ssh_string(&blob, &mut cursor)?;
    if algorithm != b"ssh-ed25519" || key.len() != 32 || cursor != blob.len() {
        return Err(KeyUtilityError::InvalidEd25519Blob);
    }
    Ok(blob)
}

fn read_ssh_string<'a>(input: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], KeyUtilityError> {
    let length_bytes: [u8; 4] = input
        .get(*cursor..*cursor + 4)
        .and_then(|value| value.try_into().ok())
        .ok_or(KeyUtilityError::InvalidEd25519Blob)?;
    *cursor += 4;
    let length = u32::from_be_bytes(length_bytes) as usize;
    let value = input
        .get(*cursor..*cursor + length)
        .ok_or(KeyUtilityError::InvalidEd25519Blob)?;
    *cursor += length;
    Ok(value)
}

fn decode_base64(input: &str) -> Result<Vec<u8>, KeyUtilityError> {
    if input.is_empty() || input.len() > MAX_PUBLIC_KEY_BYTES {
        return Err(KeyUtilityError::InvalidBase64);
    }
    let padding = input.bytes().rev().take_while(|byte| *byte == b'=').count();
    if padding > 2
        || input[..input.len() - padding].contains('=')
        || padding > 0 && input.len() & 3 != 0
        || (input.len() - padding) % 4 == 1
    {
        return Err(KeyUtilityError::InvalidBase64);
    }
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0_u32;
    let mut bits = 0_u8;
    for byte in input.bytes().take(input.len() - padding) {
        let value = base64_value(byte).ok_or(KeyUtilityError::InvalidBase64)? as u32;
        accumulator = (accumulator << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= if bits == 0 { 0 } else { (1_u32 << bits) - 1 };
        }
    }
    if accumulator != 0 {
        return Err(KeyUtilityError::InvalidBase64);
    }
    let expected_remainder = (input.len() - padding) % 4;
    if matches!(
        (expected_remainder, padding),
        (0, 1 | 2) | (2, 0 | 1) | (3, 0 | 2)
    ) {
        // Valid padded and unpadded representations are:
        // xxxx, xx== / xx, and xxx= / xxx.
    } else if expected_remainder != 0 || padding != 0 {
        return Err(KeyUtilityError::InvalidBase64);
    }
    Ok(output)
}

fn base64_value(value: u8) -> Option<u8> {
    match value {
        b'A'..=b'Z' => Some(value - b'A'),
        b'a'..=b'z' => Some(value - b'a' + 26),
        b'0'..=b'9' => Some(value - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn encode_base64_no_padding(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(TABLE[(third & 0x3f) as usize] as char);
        }
    }
    output
}

fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = if quote == Some(character) {
                None
            } else if quote.is_none() {
                Some(character)
            } else {
                quote
            };
        } else if character == '#' && quote.is_none() {
            return &line[..index];
        }
    }
    line
}

fn split_directive(value: &str) -> Option<(&str, &str)> {
    let boundary = value
        .char_indices()
        .find(|(_, character)| character.is_whitespace() || *character == '=')
        .map(|(index, _)| index)?;
    let keyword = &value[..boundary];
    let remainder = value[boundary..]
        .trim_start_matches(|character: char| character.is_whitespace() || character == '=')
        .trim();
    (!keyword.is_empty() && !remainder.is_empty()).then_some((keyword, remainder))
}

fn split_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            } else {
                current.push(character);
            }
            continue;
        }
        if character.is_whitespace() && quote.is_none() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn valid_config_value(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && !value
            .chars()
            .any(|character| character == '\0' || character.is_control())
}

fn is_concrete_alias(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && !value.starts_with('!')
        && !value.contains(['*', '?'])
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh_blob(algorithm: &[u8], key: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&(algorithm.len() as u32).to_be_bytes());
        output.extend_from_slice(algorithm);
        output.extend_from_slice(&(key.len() as u32).to_be_bytes());
        output.extend_from_slice(key);
        output
    }

    fn valid_public_key(comment: &str) -> String {
        let blob = ssh_blob(b"ssh-ed25519", &[7_u8; 32]);
        format!("ssh-ed25519 {} {comment}", encode_base64_no_padding(&blob))
    }

    #[test]
    fn validates_and_canonicalizes_ed25519_public_keys() {
        let input = valid_public_key("  RemoteDeck\t研究 主机  ");
        let canonical = canonical_ed25519_public_key(&input).expect("canonical");
        assert!(canonical.starts_with("ssh-ed25519 "));
        assert!(canonical.ends_with("RemoteDeck 研究 主机"));
        let fingerprint = ed25519_sha256_fingerprint(&canonical).expect("fingerprint");
        assert!(fingerprint.starts_with("SHA256:"));
        assert!(!fingerprint.contains('='));
    }

    #[test]
    fn rejects_algorithm_and_blob_mismatches() {
        let wrong_algorithm = encode_base64_no_padding(&ssh_blob(b"ssh-rsa", &[7_u8; 32]));
        assert_eq!(
            canonical_ed25519_public_key(&format!("ssh-ed25519 {wrong_algorithm}")),
            Err(KeyUtilityError::InvalidPublicKey)
        );
        let wrong_length = encode_base64_no_padding(&ssh_blob(b"ssh-ed25519", &[7_u8; 31]));
        assert_eq!(
            canonical_ed25519_public_key(&format!("ssh-ed25519 {wrong_length}")),
            Err(KeyUtilityError::InvalidPublicKey)
        );
        assert!(parse_authorized_key("ssh-ed25519 not!base64").is_none());
    }

    #[test]
    fn merges_by_material_and_preserves_existing_options() {
        let key = valid_public_key("new comment");
        let parsed = parse_authorized_key(&key).expect("parse");
        let existing = format!(
            "# managed elsewhere\r\nfrom=\"10.0.0.0/8\" {} old comment   \r\n\r\n",
            parsed.material
        );
        let merged = merge_authorized_key(&existing, &key).expect("merge");
        assert!(merged.already_present);
        assert_eq!(merged.content.matches(&parsed.public_key_base64).count(), 1);
        assert!(merged.content.contains("from=\"10.0.0.0/8\""));
        assert!(!merged.content.contains('\r'));
    }

    #[test]
    fn appends_a_missing_key_exactly_once() {
        let key = valid_public_key("first\ncomment");
        let first = merge_authorized_key("# keys\n", &key).expect("first");
        assert!(!first.already_present);
        assert!(first.content.ends_with("first comment\n"));
        let second = merge_authorized_key(&first.content, &key).expect("second");
        assert!(second.already_present);
        assert_eq!(second.content, first.content);
    }

    #[test]
    fn cleans_comments_with_a_unicode_character_bound() {
        let input = format!("  alpha\0 beta\n{}  ", "界".repeat(300));
        let cleaned = clean_key_comment(&input);
        assert!(!cleaned.chars().any(char::is_control));
        assert!(cleaned.chars().count() <= MAX_COMMENT_CHARS);
        assert!(cleaned.starts_with("alpha beta"));
    }

    #[test]
    fn parses_concrete_openssh_hosts_without_following_includes() {
        let config = r#"
            Host jump
              HostName jump.example.test
              User gateway
              Port 2222

            Host lab lab-copy *.wild !blocked
              HostName "lab.example.test" # outside quote
              User researcher
              IdentityFile "~/.ssh/id lab"
              IdentitiesOnly yes
              ProxyJump jump
              ServerAliveInterval 30
              ServerAliveCountMax 4
              TCPKeepAlive no
              ConnectTimeout 12
              Compression yes
              LocalForward 127.0.0.1:8080 127.0.0.1:80
              RemoteForward 127.0.0.1:17890 127.0.0.1:7890
              Include ~/.ssh/other.conf
              PermitLocalCommand yes
        "#;
        let hosts = parse_open_ssh_config(config).expect("parse");
        assert_eq!(hosts.len(), 3);
        let lab = hosts.iter().find(|host| host.alias == "lab").expect("lab");
        assert_eq!(lab.hostname, "lab.example.test");
        assert_eq!(lab.username.as_deref(), Some("researcher"));
        assert_eq!(lab.identity_file.as_deref(), Some("~/.ssh/id lab"));
        assert_eq!(lab.proxy_jump.as_deref(), Some("jump"));
        assert_eq!(lab.server_alive_count_max, Some(4));
        assert_eq!(lab.tcp_keep_alive, Some(false));
        assert_eq!(lab.local_forwards.len(), 1);
        assert_eq!(lab.remote_forwards.len(), 1);
        assert_eq!(lab.unsupported_lines.len(), 2);
        assert_eq!(lab.local_forward_count, 1);
        assert_eq!(lab.remote_forward_count, 1);
        assert_eq!(lab.unsupported_count, 2);
        assert!(
            lab.unsupported_lines
                .iter()
                .any(|line| line.contains("Include"))
        );
    }

    #[test]
    fn invalid_directive_values_are_retained_for_review() {
        let config = "Host lab\n Port 0\n TCPKeepAlive maybe\n ConnectTimeout 9999\n";
        let hosts = parse_open_ssh_config(config).expect("parse");
        assert_eq!(hosts[0].port, 22);
        assert_eq!(hosts[0].unsupported_lines.len(), 3);
        assert_eq!(hosts[0].unsupported_count, 3);
    }

    #[test]
    fn parser_enforces_input_bounds() {
        assert_eq!(
            parse_open_ssh_config(&"x".repeat(MAX_CONFIG_BYTES + 1)),
            Err(KeyUtilityError::ConfigTooLarge)
        );
        let long_line = format!("Host lab\n#{}", "x".repeat(MAX_CONFIG_LINE_BYTES + 1));
        assert_eq!(
            parse_open_ssh_config(&long_line),
            Err(KeyUtilityError::ConfigLineTooLong)
        );
    }

    #[test]
    fn parser_bounds_per_alias_retained_diagnostics() {
        let aliases = (0..MAX_HOST_CANDIDATES)
            .map(|index| format!("h{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let unsupported = format!("UnknownOption {}\n", "x".repeat(4_000)).repeat(300);
        let config = format!("Host {aliases}\n{unsupported}");
        let hosts = parse_open_ssh_config(&config).expect("bounded parse");
        assert_eq!(hosts.len(), MAX_HOST_CANDIDATES);
        assert!(hosts.iter().all(|host| host.unsupported_count == 300));
        assert!(hosts.iter().all(|host| {
            host.unsupported_lines.len() <= MAX_RETAINED_LINES_PER_KIND
                && host
                    .unsupported_lines
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
                    <= MAX_RETAINED_BYTES_PER_KIND
        }));
    }
}
