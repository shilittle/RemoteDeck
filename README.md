# RemoteDeck 2

[简体中文](#简体中文) · [English](#english) · [Releases](https://github.com/shilittle/RemoteDeck/releases) · [用户指南](docs/user-guide.zh-CN.md)

RemoteDeck 2 是面向 Windows 10/11 x64 的轻量 Linux SSH 工作台。桌面端已完整迁移到 Tauri 2 + Rust，使用系统 WebView2 与 Windows OpenSSH，不再打包 Electron、Chromium、Node.js 或 `ssh2`。

RemoteDeck 2 is a lightweight Linux SSH workspace for Windows 10/11 x64. Its desktop runtime is fully migrated to Tauri 2 and Rust, using system WebView2 and Windows OpenSSH instead of bundling Electron, Chromium, Node.js, or `ssh2`.

## 简体中文

### 下载

RemoteDeck 2 的正式发布完成后，从 [GitHub Releases](https://github.com/shilittle/RemoteDeck/releases) 下载以下文件。发布前以 [发布清单](docs/release-checklist.md)中的未勾选状态和 [待填写制品记录](docs/release-manifest.md)为准：

| 文件 | 用途 |
| --- | --- |
| `RemoteDeck-<版本>-win-x64-setup.exe` | 当前用户范围的 NSIS 安装器 |
| `SHA256SUMS.txt` | 安装器 SHA-256 校验值 |
| `release-manifest.json` | 版本、平台、大小与哈希清单 |

RemoteDeck 2 的发布目标只有 NSIS 安装包，不再发布会重复携带运行时的 portable 文件。小于 40 MiB 是正式制品门禁，并非尚未构建制品的预先结论；若系统缺少 WebView2，安装器配置会通过微软引导程序下载。仓库当前未配置 Authenticode 签名，使用正式制品前应检查发布记录中的签名状态，仅从本仓库下载并核对 SHA-256。

### 系统要求

- Windows 10/11 x64，启用 Windows OpenSSH Client。
- 远端为运行 OpenSSH Server 的 Linux。
- 结构化监控需要远端 `python3`；GPU 与 btop 能力会显式降级。
- 终端中的密码、键盘交互回答和私钥口令通过带单次票据的本机回环 WebSocket 直接进入 ConPTY，不会作为 Tauri invoke 参数或持久化数据。
- SFTP、隧道、遥测和后台命令使用 OpenSSH 非交互模式，需要可用的私钥或 ssh-agent。交互认证请在终端内完成。

### 完整功能

- 主机分组、搜索、OpenSSH config 安全导入、基于已保存直连主机的严格 ProxyJump 与高级连接选项。
- 应用独占 `known_hosts`；首次信任必须核验 SHA-256，密钥变化硬阻断并要求显式移除旧记录。
- Ed25519 密钥生成、私钥发现与幂等 `authorized_keys` 部署。
- 基于 ConPTY 与 xterm.js 的多标签真实 SSH 终端，支持 Unicode/IME、搜索、尺寸同步、重连、带来源/会话绑定的回环输入通道与自有会话清理。
- SFTP 浏览、创建、重命名、递归删除，文件/目录选择与拖放上传，以及递归传输、事务化覆盖、冲突策略、进度、取消和按当前主机配置重试。
- 独立 LocalForward/RemoteForward 隧道，含健康状态、修订序事件、有界日志、按当前主机配置退避重连和托管进程清理。
- 不在远端落盘的 Python JSONL 遥测、有界历史、GPU/进程视图、含进程启动时钟复验与原生确认的 TERM/KILL 门禁，以及安装级所有权的 btop watchdog。
- Rust 端 L0/L1/L2 命令复核、后台任务与 PTY 分流；Codex、Claude Code、Gemini CLI 和 OpenCode 的探测、安装/更新、登录、启动及 tmux 恢复。
- 首次向导、统一任务中心、托盘常驻、登录启动、单实例恢复、脱敏诊断包。
- 对 LabPulse SSH v0.1.0 与 RemoteDeck v1 数据进行预览、选择性、事务化、哈希幂等迁移；不会导入旧主机信任。

### 开发与发布验证

```powershell
corepack enable
pnpm install --frozen-lockfile
pnpm verify
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml --all -- --check
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --all-features
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --all-features -- -D warnings -A linker-messages
pnpm dist:win
```

架构、安全边界和发布门禁见 [Tauri 2 重构说明](docs/tauri2-rewrite.md)、[架构](docs/architecture.md)、[测试](docs/testing.md)与[中文用户指南](docs/user-guide.zh-CN.md)。

## English

### Download

After the RemoteDeck 2 release is complete, download these assets from [GitHub Releases](https://github.com/shilittle/RemoteDeck/releases). Until then, the unchecked [release checklist](docs/release-checklist.md) and [pending artifact record](docs/release-manifest.md) are authoritative:

| File | Purpose |
| --- | --- |
| `RemoteDeck-<version>-win-x64-setup.exe` | Per-user NSIS installer |
| `SHA256SUMS.txt` | Installer SHA-256 checksum |
| `release-manifest.json` | Version, platform, size, and digest metadata |

RemoteDeck 2 targets only an NSIS installer; it no longer publishes a portable artifact that duplicates the desktop runtime. Below 40 MiB is a gate for the exact release artifact, not a result asserted before that artifact exists. The installer configuration uses Microsoft's bootstrapper when WebView2 is absent. The repository currently configures no Authenticode signing; inspect the recorded signature status, download only from this repository, and verify SHA-256 before using a published artifact.

### Requirements

- Windows 10/11 x64 with Windows OpenSSH Client enabled.
- A Linux target running OpenSSH Server.
- Remote `python3` for structured telemetry; GPU and btop features degrade explicitly.
- Passwords, keyboard-interactive answers, and key passphrases travel from xterm.js to ConPTY through a single-use-ticket authenticated loopback WebSocket; they are never Tauri invoke payloads or persistent data.
- SFTP, tunnels, telemetry, and background commands use OpenSSH batch mode and therefore require a working private key or ssh-agent. Use a terminal for interactive authentication.

### Complete feature set

- Grouped/searchable hosts, safe OpenSSH-config import, strict saved-profile ProxyJump, and advanced connection options.
- An app-owned `known_hosts`; explicit SHA-256 verification on first use, hard failure on changed keys, and explicit trust removal.
- Ed25519 generation, private-key discovery, and idempotent `authorized_keys` deployment.
- Real multi-tab SSH terminals through ConPTY and xterm.js, including Unicode/IME, search, resize synchronization, reconnect, a session/origin-bound loopback input channel, and owned-session cleanup.
- SFTP browse/create/rename/recursive-delete, native file/folder selection and drag-in upload, plus recursive transfers, transactional overwrite, conflict policies, progress, cancellation, and retry against the current saved host profile.
- Dedicated LocalForward/RemoteForward processes with health state, revisioned events, bounded logs, reconnect against the current saved host profile, and owned-process cleanup.
- Non-persistent Python JSONL telemetry, bounded history, GPU/process views, process start-time identity revalidation, native TERM/KILL escalation confirmation, and an installation-owned btop watchdog.
- Rust-enforced L0/L1/L2 command review, bounded background jobs, and PTY handoff; probe/install/update/login/start/tmux-resume flows for Codex, Claude Code, Gemini CLI, and OpenCode.
- First-run onboarding, unified task center, tray keepalive, launch at login, single-instance restore, and redacted diagnostics.
- Previewed, selective, transactional, hash-idempotent migration from LabPulse SSH v0.1.0 and RemoteDeck v1; legacy host trust is deliberately not imported.

See the [English user guide](docs/user-guide.en.md), [architecture](docs/architecture.md), [security model](docs/security.md), and [testing guide](docs/testing.md).

The original LabPulse PowerShell application remains under [`legacy/labpulse-v0.1.0`](legacy/labpulse-v0.1.0/README.md). RemoteDeck is licensed under the [MIT License](LICENSE).
