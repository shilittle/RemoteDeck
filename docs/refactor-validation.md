# WebUI 重构验收记录

日期：2026-09-08。本记录保留 WebUI 重构首次完成验收时的结果，基于 `7a88b4163abc4b6b27c41348f3eb0d03a49be5bc`，尚未提交或发布。后续禁止控制台弹窗的修复、测试及替代安装包见[隐藏进程验收记录](windowless-processes.md)。下列结果不代表未执行的人工、真实账户或 CI 环境验收。

## 实现

- `crates/remotedeck-core` 承接 SSH、主机信任、PTY、SFTP、命令、监控、隧道、密钥、Agent 和迁移业务，移除 Tauri 类型，通过事件接口输出状态。
- `apps/server` 是 Axum 本机服务和 Windows 适配层，内嵌静态资源、动态绑定 `127.0.0.1`，处理单实例、浏览器票据、原生选择框、后台启动及有界停止。
- `apps/web` 使用 React、xterm.js 和普通 CSS，主导航为“主机、工作区、任务、设置”。所有生产操作使用显式 API；终端走 WebSocket，状态走 SSE。
- 生产源码、依赖和发布链已移除 Tauri/WebView2。保留原有 v2 数据目录、备份、专属 known_hosts、btop 所有权和选择性迁移。原工作树的遥测、依赖及 CI 改动已承接。

## 自动验收

| 项目 | 结果 |
| --- | --- |
| `pnpm verify` | 通过：lint、类型检查、前端测试、接口集成、Rust fmt/test/严格 Clippy、基础浏览器用例及生产构建 |
| WebUI 单元测试 | 13 个文件、47 项通过 |
| Rust 常规测试 | core 165 项、server 32 项、Windows CLI 1 项，共 198 项通过；5 项外部夹具在普通测试中忽略，已单独执行 |
| 真实 OpenSSH | 5 项通过，包括首次信任/变化拒绝、ProxyJump、递归 Unicode SFTP、双向覆盖、取消后重试、双向 TCP 转发及 Windows `ssh -G` |
| 真实浏览器＋OpenSSH | 3 项通过：主机保存与未信任错误、指纹接受、真实 PTY 输入、刷新、命令确认和结果、真实本机文件上传/下载、隧道、终端关闭 |
| 浏览器恢复与并发 | 真实 PTY 保留原会话 ID；断线故障注入后携带原 lease 自动附着；多页面独占接管；关闭页面再打开后输出恢复且没有重复回放 |
| 安全与状态回归 | Host/Origin/CSRF、未认证请求、票据重放/过期/浏览器绑定、请求去重、资源修订号、代次、旧 lease、输出上限及停止准入均有自动回归 |
| 开发入口 | `REMOTEDECK_DEV_SMOKE=1 pnpm dev` 通过：构建、Vite＋真实服务、API 和停止 |
| 依赖与仓库检查 | 冻结锁文件安装、`pnpm audit`、`cargo audit` 通过；工作流 YAML 解析与固定 action SHA 长度检查通过 |

普通 `pnpm verify` 未启动 Docker，因此其中两项 SSH 浏览器用例会跳过；完整运行通过单独的 `pnpm test:e2e:openssh` 完成，不能把普通运行的跳过计为通过。故障注入只中断测试浏览器与真实服务之间的连接，所有终端、命令、文件和隧道业务仍由真实 OpenSSH 执行。

本机 Docker Hub 访问失败，本次夹具使用官方 ECR 镜像 `public.ecr.aws/docker/library/ubuntu:24.04`，并强制与默认镜像使用同一个固定 SHA-256。夹具 sshd 使用 Curve25519 KEX 兼容本机 Windows OpenSSH 9.5p1；这没有修改用户或生产主机的 SSH 配置，也没有修复上游 ssh-keyscan 问题。

本机日志位于 `.cache/refactor-verify-final.log`、`.cache/openssh-browser-validation-v7.log`。界面截图 `.cache/webui-terminal-recovered.png` 已检查；日志和截图属于本机验收材料，不进入生产安装包。

## 安装包

`pnpm dist:win` 与 Windows PowerShell 5.1 下的 `scripts/verify-release.ps1` 均完整成功退出。

| 字段 | 结果 |
| --- | --- |
| 文件 | `dist/RemoteDeck-2.0.1-win-x64-setup.exe` |
| 大小 | 1,380,746 字节，约 1.32 MiB，低于 40 MiB |
| SHA-256 | `8e5582feaf213452737dc144ebff2d3825355025cf15fa9d7ed8d1d8c8b0c90e` |
| Authenticode | `NotSigned`，未签名 |
| 安装与运行 | 在含空格的隔离路径中静默安装；真实 `/health`、独立 PID、内嵌 WebUI、一次性票据交换和已认证 bootstrap 均通过 |
| 停止与卸载 | 生产 `--stop` 入口成功；进程退出且 `last-shutdown.json.clean=true`；静默卸载通过，隔离用户数据保留 |
| 原环境恢复 | 当前用户注册表和快捷方式恢复；曾中断的验收恢复步骤已核对，Run 注册表导出与备份的 SHA-256 一致；最终完整重跑成功 |

对应本机日志为 `.cache/package-win-final.log` 与 `.cache/release-acceptance-final.log`。`dist/release-manifest.json` 和 `dist/SHA256SUMS.txt` 与上述制品一致。

## 尚未完成的验收

- 原生 Windows 文件/目录选择框的人工点选、登录后自动启动和真实旧版 Tauri 安装的并发/升级操作；已有对应适配和单元测试，尚未在干净 Windows 虚拟机中人工走完。
- Codex、Claude、Gemini、OpenCode 的真实账户登录、安装更新、会话恢复；这些需要可用账户、网络与远端权限。现有命令规划、风险确认和 tmux 生命周期实现及回归仍保留。
- 真实 GPU、btop 及缺少远端工具的完整监控矩阵；现有解析、降级、身份复验和所有权逻辑有单元回归。
- 在真实 sshd 上注入传输提升失败验证覆盖回滚，以及远端 SSH 连接/隧道故障后的完整恢复矩阵。当前真实夹具已验证正常覆盖和取消重试，事务回滚与重连逻辑由单元回归覆盖。
- 修改后的 GitHub Actions 尚未在远端 CI 执行；未签名、未推送、未发布。

这些项目保持未验收，不以接口测试或本机成功记录代替。
