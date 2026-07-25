# Security policy / 安全策略

[简体中文](#简体中文) · [English](#english)

## 简体中文

### 支持范围

安全修复面向最新 GitHub Release 和 `main`。旧版 LabPulse SSH v0.1.0 只为兼容迁移与历史保留，不再接收功能更新。

### 报告漏洞

请优先使用仓库 **Security** 页的 **Report a vulnerability** 私密报告功能。不要在公开 Issue、讨论、日志或截图中发布：

- 密码、私钥、私钥口令或 SSH agent 内容。
- 主机地址、用户名、真实主机指纹或组织内部路径。
- Codex/OpenAI token、`~/.codex/auth.json` 或其他账户凭据。
- 代码签名证书、私钥、Base64 PFX 或证书口令。

报告中请提供受影响版本、Windows/远端 Linux 版本、可最小化的复现步骤、预期与实际结果，以及已经脱敏的诊断信息。维护者会先确认收到，再评估影响与修复计划。请在修复发布前避免公开披露可利用细节。

如果你怀疑凭据已经暴露，请先在对应系统中撤销/轮换凭据；RemoteDeck 维护者无法替你恢复第三方账户。

## English

### Supported versions

Security fixes target the latest GitHub Release and `main`. LabPulse SSH v0.1.0 is retained only for migration compatibility and history and does not receive feature updates.

### Reporting a vulnerability

Prefer **Report a vulnerability** on the repository's **Security** page. Do not place any of the following in a public Issue, discussion, log, or screenshot:

- Passwords, private keys, key passphrases, or SSH agent contents.
- Host addresses, usernames, real host fingerprints, or internal organization paths.
- Codex/OpenAI tokens, `~/.codex/auth.json`, or other account credentials.
- Code-signing certificates, private keys, Base64 PFX content, or certificate passwords.

Include the affected version, Windows and remote Linux versions, minimized reproduction steps, expected and actual results, and only redacted diagnostics. Maintainers will acknowledge the report before assessing impact and remediation. Avoid public disclosure of exploitable details until a fix is available.

If credentials may already be exposed, revoke or rotate them in the owning system first. RemoteDeck maintainers cannot recover third-party accounts on your behalf.
