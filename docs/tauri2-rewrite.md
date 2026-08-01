# RemoteDeck 2 — Tauri 2 原生重构

## 架构决策

RemoteDeck 2 已从 Electron 43 全量迁移到 Tauri 2。生产打包目标由一个 Rust 可执行文件、静态 React 资源和 Windows WebView2 组成；运行时不再配置携带 Node.js、Electron、`ssh2` 或通用 shell/文件系统插件。

Windows 打包配置只允许当前用户范围的 NSIS 目标，并使用 WebView2 `downloadBootstrapper`。因此该目标不配置内嵌百兆级浏览器运行时，正式制品门禁为小于 40 MiB。

## 原生模块

- Rust 持久化层：版本化状态、原子替换、备份恢复、严格输入验证，以及先退役运行时、再删除主机及其隧道/命令预设的协调流程；被其他主机用作 ProxyJump 时拒绝删除。
- Windows OpenSSH：应用独占 `known_hosts`、首次显式信任、主机密钥变化硬阻断、`-F none` 配置隔离和有限输出。
- ConPTY 终端：多会话、Unicode/IME、搜索、尺寸同步、重连、自有进程清理，以及通过单次票据、来源/会话代际绑定的 IPv4 回环 WebSocket 输入通道。
- SFTP 与传输：浏览、目录操作、递归上传下载、事务化覆盖/回滚、冲突策略、进度、取消、按当前主机配置重试、主机退役屏障和临时文件归属校验。
- 隧道监督器：LocalForward/RemoteForward、健康检查、修订序状态、有界日志、按当前主机配置自动恢复、退避重连和活跃路由编辑门禁。
- 遥测：不在远端落盘的 Python JSONL 采集器、有界历史与修订序状态、CPU/内存/网络/磁盘/GPU/进程视图、基于进程启动时钟的身份复验、原生 KILL 确认，以及安装级稳定所有权/30 秒启动租约的 btop watchdog。
- 命令与 Agent：Rust L0/L1/L2 风险门禁、有限输出后台任务、PTY 分流，以及 Codex、Claude、Gemini、OpenCode 的普通权限/tmux 工作流。
- 桌面生命周期：首次向导、任务中心、托盘保活、登录启动、单实例恢复、脱敏诊断包。
- 迁移：预览并显式导入 LabPulse SSH v0.1.0 或 RemoteDeck v1 数据；来源哈希防止重复导入。

## 安全边界

渲染器只能够调用 `src-tauri/src/lib.rs` 注册的窄命令。它没有通用进程、shell、文件系统、网络或 HTTP 能力。终端输入是唯一的专用网络通道：CSP 仅允许 IPv4 回环 WebSocket，握手校验精确 host、path、来源、短期单次票据与会话代际，帧和连接数均有上限。密码、键盘交互回答和私钥口令只停留在 OpenSSH/PTY 内，不作为 Tauri invoke 参数，也不会进入配置、日志或诊断包。

所有 SSH、SFTP、命令、遥测与隧道连接都使用应用独占的可信主机密钥文件。主机密钥发生变化时，用户必须先删除旧记录、通过独立渠道核验新指纹，再重新接受。

## 本地验证

Windows 10/11 x64 需要 Node.js 24、pnpm 11、stable Rust、Microsoft C++ Build Tools、WebView2 和 Windows OpenSSH Client。

```powershell
pnpm install --frozen-lockfile
cargo install tauri-cli --version 2.11.4 --locked
pnpm verify
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --all-features
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets --all-features -- -D warnings -A linker-messages
pnpm dist:win
```

构建成功后，NSIS 安装器预期输出在 `apps/desktop/src-tauri/target/release/bundle/nsis/`。该路径说明不是制品已生成的声明；正式发布还必须完成干净安装、窗口启动、静默卸载、签名状态记录、SHA-256、独立回下载校验和 40 MiB 大小门禁。
