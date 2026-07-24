# RemoteDeck

RemoteDeck 是面向 Windows 10/11 x64 的独立 SSH 工作台。它把主机与密钥管理、可信主机指纹、真实多标签终端、SFTP、LocalForward/RemoteForward、Linux 遥测、进程操作、命令风险确认和远端 Codex CLI 工作流放在一个安全边界清晰的桌面应用中。

RemoteDeck v1.0.0 不要求用户安装 Node.js。发行包提供可选择安装目录的 NSIS 安装器和单文件 portable 版本；开发仓库仍使用 Node.js 24 与 pnpm 11。

## 主要能力

- ssh2 驱动的密码、交互问答、私钥和 Windows OpenSSH agent 认证；首次指纹确认、变更硬失败、单层 ProxyJump。
- xterm.js 真 PTY，多主机标签、Unicode/IME、尺寸同步、搜索、受限 HTTP(S) 链接和剪贴板。
- SFTP 目录树、CRUD、递归上传/下载、100 MiB 级进度、冲突策略、取消与临时文件原子提交。
- 独立 SSH 连接承载本地/远程隧道，带健康检查、网络恢复和只读 Clash/Mihomo 候选探测。
- 通过 SSH stdin 运行、不落盘的 Python JSONL collector，提供系统/GPU/进程图表、保留窗口、自动恢复与 btop watchdog。
- 全局/主机命令库与主进程 L0/L1/L2 风险复核；远端 Codex 安装、登录、启动、恢复和更新都进入普通 SSH PTY。
- 首次引导、LabPulse SSH v0.1.0 显式迁移、任务中心、托盘常驻、登录启动和一键脱敏诊断包。

旧版 PowerShell 与原始历史完整保留在 [`legacy/labpulse-v0.1.0`](legacy/labpulse-v0.1.0/README.md)，新功能没有继续堆入旧单文件脚本。

## 安装与使用

从发布候选中选择：

- `RemoteDeck-1.0.0-win-x64-setup.exe`：当前用户安装，可选择目录并创建开始菜单/桌面快捷方式。
- `RemoteDeck-1.0.0-win-x64-portable.exe`：直接运行；配置仍保存到当前 Windows 用户的应用数据目录。

首次启动按引导添加主机，确认服务端显示的 SHA-256 指纹，输入一次性凭据并连接。密码、交互回答和私钥口令只在内存中使用，不会写入 profiles 或日志。完整操作见 [`docs/user-guide.md`](docs/user-guide.md)。

> v1.0.0 候选未附代码签名证书，Windows SmartScreen 可能显示未知发布者。请在使用前核对发布清单中的 SHA-256。

## 开发与验证

```powershell
pnpm install --frozen-lockfile
pnpm verify
pnpm test:e2e
pnpm dist:win
```

`pnpm verify` 依次执行 lint、TypeScript、单元测试、集成测试和生产 build。Windows 打包、净安装与 Docker OpenSSH 测试也固化在 CI。详情见 [`docs/testing.md`](docs/testing.md) 与 [`docs/release-checklist.md`](docs/release-checklist.md)。

## 文档

- [用户指南](docs/user-guide.md)
- [架构](docs/architecture.md)
- [安全模型](docs/security.md) 与 [v1.0 安全审计](docs/security-audit.md)
- [SSH](docs/ssh.md)、[终端](docs/terminal.md)、[SFTP](docs/sftp.md)、[隧道](docs/tunnels.md)、[监控](docs/monitoring.md)
- [命令与 Codex](docs/commands-codex.md)、[迁移与桌面体验](docs/experience-and-migration.md)
- [测试矩阵](docs/testing.md)、[已知限制](docs/known-limitations.md)、[变更记录](CHANGELOG.md)

License: MIT.
