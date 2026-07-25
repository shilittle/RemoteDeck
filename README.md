# RemoteDeck

[简体中文](#简体中文) · [English](#english) · [Releases](https://github.com/shilittle/RemoteDeck/releases) · [User Guide / 用户指南](docs/user-guide.md)

RemoteDeck is a security-focused Windows SSH workspace for Linux servers. It combines trusted host-key handling, real terminal sessions, SFTP, tunnels, monitoring, safe command execution, and remote Codex CLI workflows in one standalone Electron application.

RemoteDeck 是面向 Linux 服务器的安全型 Windows SSH 工作台，将可信主机密钥、真实终端、SFTP、隧道、监控、安全命令执行和远端 Codex CLI 工作流整合到独立 Electron 桌面应用中。

## 简体中文

### 下载与系统要求

- 本地系统：Windows 10/11 x64。
- 远端系统：运行 OpenSSH Server 的 Linux；结构化监控需要 `python3`。
- 最终用户无需安装 Node.js、pnpm 或 Electron。
- 从 [GitHub Releases](https://github.com/shilittle/RemoteDeck/releases) 下载 NSIS 安装器或单文件 portable 版本。

| 文件 | 用途 |
| --- | --- |
| `RemoteDeck-<版本>-win-x64-setup.exe` | 当前用户安装，可选择安装目录并创建快捷方式 |
| `RemoteDeck-<版本>-win-x64-portable.exe` | 直接运行；配置保存在当前 Windows 用户的应用数据目录 |
| `RemoteDeck-<版本>-win-x64-setup.exe.blockmap` | 发布/更新元数据，普通用户无需手动打开 |

> 发布页会列出 SHA-256 与代码签名状态。只有 Authenticode 验证为 `Valid`、发布者与发布说明一致的构建才会标记为“已签名”。v1.0.0 初始资产未签名，Windows SmartScreen 可能显示未知发布者。

### 核心能力

- 密码、键盘交互、加密私钥和 Windows OpenSSH agent 认证。
- 未知主机密钥必须显式接受；主机密钥变化会硬失败并同时显示新旧指纹。
- 单层 ProxyJump、幂等 OpenSSH `Include`、Ed25519 生成与公钥原子部署。
- 基于 xterm.js 的真实多标签 SSH PTY，支持 Unicode/IME、搜索、尺寸同步和 Ctrl+C。
- SFTP 浏览、创建、重命名、删除、递归传输、冲突策略、进度、取消和重试。
- 独立 SSH 连接承载 LocalForward/RemoteForward，具备健康检查和网络恢复。
- 不在远端落盘的 Python JSONL 遥测、进程控制、GPU 降级和 btop watchdog。
- 主进程执行 L0/L1/L2 风险复核；Codex 安装、登录与交互始终进入普通 SSH PTY/tmux。
- 首次引导、LabPulse SSH v0.1.0 显式迁移、任务中心、托盘常驻和脱敏诊断包。

### 快速开始

1. 从 Release 下载并验证 Authenticode 签名和 SHA-256。
2. 启动 RemoteDeck，选择“添加主机”或“迁移 LabPulse SSH v0.1.0”。
3. 填写主机地址、端口、用户名、认证方式和可选工作目录。
4. 通过独立可信渠道核对首次显示的 SHA-256 主机指纹，然后接受。
5. 连接后使用终端、文件、隧道、监控和命令工作区。

密码、键盘交互回答和私钥口令只驻留内存，不写入配置、日志或诊断包。完整说明见[中文用户指南](docs/user-guide.zh-CN.md)。

### 开发与验证

```powershell
corepack enable
pnpm install --frozen-lockfile
pnpm verify
pnpm test:e2e
pnpm dist:win
```

签名发布还需要受信任的 Authenticode `.pfx/.p12` 证书：

```powershell
$env:WIN_CSC_LINK = '<证书路径或 Base64>'
$env:WIN_CSC_KEY_PASSWORD = '<证书口令>'
pnpm dist:win:signed
pnpm verify:signatures
```

证书和口令不得提交到仓库。详细流程见[代码签名指南](docs/code-signing.md)。

## English

### Downloads and requirements

- Local platform: Windows 10/11 x64.
- Remote platform: Linux with OpenSSH Server; structured monitoring requires `python3`.
- End users do not need Node.js, pnpm, or Electron.
- Download the NSIS installer or the single-file portable build from [GitHub Releases](https://github.com/shilittle/RemoteDeck/releases).

| File | Purpose |
| --- | --- |
| `RemoteDeck-<version>-win-x64-setup.exe` | Per-user installer with a selectable destination and shortcuts |
| `RemoteDeck-<version>-win-x64-portable.exe` | Runs directly; settings remain in the current Windows user's application-data directory |
| `RemoteDeck-<version>-win-x64-setup.exe.blockmap` | Release/update metadata; users do not open it manually |

> Every release states its SHA-256 values and signing status. A build is described as signed only when Authenticode reports `Valid` and the publisher matches the release notes. The initial v1.0.0 assets are unsigned, so Windows SmartScreen may show an unknown-publisher warning.

### Highlights

- Password, keyboard-interactive, encrypted private-key, and Windows OpenSSH agent authentication.
- Explicit first-use host-key trust; changed keys hard-fail and show both fingerprints.
- One-level ProxyJump, idempotent OpenSSH `Include`, Ed25519 generation, and atomic public-key deployment.
- Real multi-tab xterm.js SSH PTYs with Unicode/IME, search, resize synchronization, and Ctrl+C.
- SFTP browse/create/rename/delete, recursive transfers, conflict policies, progress, cancellation, and retry.
- LocalForward/RemoteForward over dedicated SSH connections with health checks and network recovery.
- Non-persistent Python JSONL telemetry, process controls, explicit GPU degradation, and a btop watchdog.
- Main-process L0/L1/L2 command review; Codex install, login, and interaction remain in standard SSH PTYs/tmux.
- First-run onboarding, explicit LabPulse SSH v0.1.0 migration, task center, tray keepalive, and redacted diagnostics.

### Quick start

1. Download a Release and verify its Authenticode signature and SHA-256.
2. Start RemoteDeck and choose either “Add host” or “Migrate LabPulse SSH v0.1.0.”
3. Enter the host, port, username, authentication method, and optional working directory.
4. Compare the first-use SHA-256 host fingerprint through an independent trusted channel, then accept it.
5. Use the terminal, files, tunnels, monitoring, and commands workspaces after connecting.

Passwords, interactive answers, and private-key passphrases remain memory-only and are excluded from configuration, logs, and diagnostics. See the complete [English user guide](docs/user-guide.en.md).

### Development and verification

```powershell
corepack enable
pnpm install --frozen-lockfile
pnpm verify
pnpm test:e2e
pnpm dist:win
```

A trusted Authenticode `.pfx/.p12` certificate is required for signed releases:

```powershell
$env:WIN_CSC_LINK = '<certificate path or Base64>'
$env:WIN_CSC_KEY_PASSWORD = '<certificate password>'
pnpm dist:win:signed
pnpm verify:signatures
```

Never commit the certificate or password. See the [code-signing guide](docs/code-signing.md).

## Documentation / 文档

- [User guide / 用户指南](docs/user-guide.md)
- [Code signing / 代码签名](docs/code-signing.md)
- [Architecture / 架构](docs/architecture.md)
- [Security policy / 安全策略](SECURITY.md), [security model / 安全模型](docs/security.md), and [v1.0 security audit / 安全审计](docs/security-audit.md)
- [SSH](docs/ssh.md), [terminal / 终端](docs/terminal.md), [SFTP](docs/sftp.md), [tunnels / 隧道](docs/tunnels.md), [monitoring / 监控](docs/monitoring.md)
- [Commands and Codex / 命令与 Codex](docs/commands-codex.md)
- [Testing / 测试](docs/testing.md), [known limitations / 已知限制](docs/known-limitations.md), [changelog / 变更记录](CHANGELOG.md)

The original LabPulse SSH PowerShell application and history remain under [`legacy/labpulse-v0.1.0`](legacy/labpulse-v0.1.0/README.md). RemoteDeck is licensed under the [MIT License](LICENSE).
