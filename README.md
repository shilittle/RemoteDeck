# RemoteDeck 2

RemoteDeck 是一个面向 Windows 10/11 x64 的本机 SSH 工作台。程序以 Rust 服务进程运行，在 `127.0.0.1` 提供简单 WebUI；启动后打开系统默认浏览器。浏览器关闭不会停止终端、文件传输、命令任务或隧道，重新打开页面即可从服务端状态恢复。

生产结构分为三层：`crates/remotedeck-core` 保存主机、信任、SSH/ConPTY、SFTP、任务、监控、隧道、Agent、迁移和诊断逻辑；`apps/server` 提供 Windows 适配、HTTP/SSE/WebSocket、生命周期和静态资源服务；`apps/web` 是 React、xterm.js 和 Vite WebUI。发布程序是 `remotedeck-server` 包生成的 `RemoteDeck.exe`，前端资源嵌入二进制，不携带 Electron、Tauri、WebView2 或生产 Node.js。

## 核心工作流

- **主机**：保存和分组主机，导入明确的 OpenSSH config，扫描并核验 SHA-256 指纹，配置 ProxyJump，发现、生成和部署密钥。
- **工作区**：打开持久 SSH/ConPTY 终端；浏览和传输 SFTP 文件；执行带风险复核的命令；启动 Codex、Claude、Gemini、OpenCode；查看监控、GPU、进程、btop 和双向隧道。
- **任务**：查看跨主机的传输、命令、监控和隧道任务，取消、重试并返回对应工作区。
- **设置**：调整终端和运行偏好、登录后启动、迁移、诊断和停止服务。

首次连接的顺序是“保存主机 → 扫描并独立核对指纹 → 在终端内完成认证 → 使用后台功能”。密码和私钥口令只进入交互式 PTY，不进入 HTTP 请求、配置、日志或诊断包。

## 安装和运行要求

[下载最新 Windows 安装包](https://github.com/shilittle/RemoteDeck/releases/latest) · [v2.1.0 更新说明](docs/releases/v2.1.0.md)。升级前请先退出旧版本；后台操作不会弹出额外的命令行窗口。

终端用户需要 Windows 10/11 x64、Windows OpenSSH Client，以及可通过 OpenSSH 访问的 Linux SSH Server。安装包是当前用户范围的 NSIS；用户数据放在 `%APPDATA%\io.github.shilittle.remotedeck`，卸载保留这些数据。安装包不捆绑 Node.js、浏览器运行时或远端依赖。

开发机需要 Node.js 24、pnpm 11、Rust 1.98.0（由 `rust-toolchain.toml` 固定）、Microsoft C++ Build Tools、NSIS `makensis.exe` 和 Docker（运行 OpenSSH 集成夹具时）。NSIS 可使用标准安装目录或已有 Tauri 缓存中的 `makensis.exe`，构建不依赖 `tauri-cli`。

## 开发命令

```powershell
pnpm install --frozen-lockfile
pnpm dev
pnpm lint
pnpm typecheck
pnpm test
pnpm test:integration
pnpm test:e2e
pnpm build
pnpm verify
pnpm dist:win

cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings -A linker-messages
```

`pnpm dist:win` 先构建 `apps/web/dist`，再调用 `scripts/package-win.ps1` 构建 `cargo build --release -p remotedeck-server` 并用 `scripts/installer.nsi` 生成 `dist/RemoteDeck-<version>-win-x64-setup.exe`。`scripts/verify-release.ps1` 使用隔离 `--data-dir` 与 `--launch-file`，请求真实 `/health`、检查独立进程存活、调用 `--stop`，再完成静默卸载和用户数据保留检查；正常 runtime descriptor 的控制令牌不会被脚本读取。

## 安全边界

服务器只监听 IPv4 loopback。启动时创建短期一次性浏览器票据，交换为 HttpOnly、SameSite 会话；受保护路由校验 Host、Origin、CSRF 和 WebSocket 票据。浏览器只使用显式 `/api/v1` 路由、SSE 事件流和终端 WebSocket，不拥有通用 shell、文件系统或代理能力。

RemoteDeck 使用应用专属 `known_hosts`，从不修改用户全局 OpenSSH 信任文件。首次或变化的主机密钥必须显示指纹、由用户明确接受并在写入前重新扫描；每条 SSH/SFTP/隧道/后台命令连接都启用严格主机密钥检查。服务只停止当前实例登记的子进程，终端输出和输入不写入日志。

## 发布状态

发布目标只有 Windows x64 current-user NSIS，安装包必须小于 40 MiB。构建会记录 SHA-256 和 Authenticode 状态；当前仓库不包含签名密钥，也不会把未观察到的签名或发布结果写成事实。CI 和 [发布清单](docs/release-checklist.md) 是候选制品的证据来源。

`pnpm verify` 运行本地完整门禁；`pnpm test:e2e:openssh` 创建隔离 OpenSSH 夹具并执行真实浏览器业务和恢复流程。原生系统选择框、第三方 Agent 账户以及真实旧版升级需要单独验收。测试结果和安装包信息记录在[发布清单](docs/release-checklist.md)、[重构验收](docs/refactor-validation.md)与[弹窗修复验收](docs/windowless-processes.md)，未执行的项目会明确列出。

## 文档

- [架构](docs/architecture.md)
- [安全模型](docs/security.md)
- [测试矩阵](docs/testing.md)
- [简体中文用户指南](docs/user-guide.zh-CN.md) / [English user guide](docs/user-guide.en.md)
- [已知限制](docs/known-limitations.md)
- [发布清单](docs/release-checklist.md) / [制品记录](docs/release-manifest.md)

`docs/tauri2-rewrite.md`、`docs/scope-v1.md`、`docs/execution-plan.md` 和 `docs/baseline-v0.1.md` 是历史审计材料。它们保留旧版本决策或基线，不描述当前生产实现。
