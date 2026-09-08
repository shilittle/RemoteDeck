# Windowless process policy

RemoteDeck's development, integration, packaging, and release-verification
scripts must never create a visible Windows console window. Output from build
tools may remain attached to the terminal that invoked the script. This policy
is about child-process creation; it does not hide a terminal that the user
explicitly opened to run a command.

The current script coverage is:

- `scripts/dev.mjs` uses one `run` wrapper for Vite, Cargo, the copied service,
  and the development UI. It inherits the caller's stdio and assigns
  `windowsHide: true` after per-call options are merged, so an option cannot
  accidentally turn the setting off.
- `scripts/run-openssh-integration.mjs` applies the same final
  `windowsHide: true` rule to Docker, `ssh-keygen`, Cargo, and Playwright.
  `scripts/run-openssh-integration.sh` only delegates to that Node runner.
- `scripts/verify-release.ps1` uses `Start-HiddenProcess` for the NSIS
  installer, the installed service, the explicit `--stop` helper, and the
  uninstaller, as well as the registry snapshot/restore calls. The helper sets
  `UseShellExecute = $false`, `CreateNoWindow = $true`, and `WindowStyle =
  Hidden`; registry output is redirected before its exit code is checked. It
  uses `ProcessStartInfo.ArgumentList`
  when available and a Windows CRT-compatible quoted-argument fallback on
  Windows PowerShell 5.1, so isolated paths containing spaces remain intact.
  The NSIS installer is the deliberate exception: its final raw argument is
  `/S /D=<path>` because NSIS requires an unquoted `/D=` value at the end of
  the command line.
- `scripts/package-win.ps1` invokes `pnpm` and Cargo in its existing PowerShell
  process with the call operator. It does not launch a second terminal or use
  `Start-Process`; build output therefore stays in the caller's terminal.
- `scripts/installer.nsi` creates shortcuts to the release executable. The
  server binary is built with the Windows GUI subsystem in both debug and
  release builds, so the installed service does not open a console window.
- Ordinary native children in `crates/remotedeck-core` and `apps/server` use
  the shared `process::command`/`process::async_command` helpers, which apply
  `CREATE_NO_WINDOW`. The only exception is the `portable-pty` ConPTY launch,
  whose pseudo-console startup is required for interactive terminal input and
  does not create an independent console window. Boundary verification rejects
  raw `Command::new` and `.creation_flags(...)` calls in `apps/server/src` and
  `apps/server/tests`; a core regression test guards the core sources.

The rule applies to future script changes as well. A new Node child launcher
must put `windowsHide: true` after spread options. A new PowerShell child
launcher must go through `Start-HiddenProcess` (or an equivalent
`ProcessStartInfo` with all three hidden-process properties); adding only
`-WindowStyle Hidden` is insufficient. The scripts do not kill unrelated
`cmd.exe` processes that may belong to the host environment or other tools.

Static checks for the current coverage:

```text
node --check scripts/dev.mjs
node --check scripts/run-openssh-integration.mjs
powershell.exe -NoProfile -NonInteractive -Command '$tokens=$null; $errors=$null; [System.Management.Automation.Language.Parser]::ParseFile("scripts/verify-release.ps1", [ref]$tokens, [ref]$errors) | Out-Null; if($errors.Count){exit 1}'
```

The repository
does not claim that arbitrary third-party launchers, historical files under
`legacy/`, or already-running external `cmd.exe` windows are controlled by
this policy.

## 2026-09-08 修复与本机验收

本次排查发现 SSH 探测与执行、遥测等后台进程没有统一设置隐藏标志。
普通进程现在统一使用 `CREATE_NO_WINDOW`；调试版和发行版主程序均为
Windows GUI subsystem。终端继续通过原有 ConPTY 工作，不单独打开 cmd 窗口。

| 检查 | 结果 |
| --- | --- |
| `pnpm verify` | 通过；前端 47 项、core 170 项、server 32 项、Windows CLI 2 项，fmt、严格 Clippy 和生产构建通过 |
| 隐藏进程专项 | 同步、Tokio 子进程的 `GetConsoleWindow()` 为 0；构建后的 debug PE subsystem 为 2；原始进程构造受源码检查约束 |
| 真实 OpenSSH | 5 项通过，包含 ProxyJump、指纹变化拒绝、递归 SFTP、覆盖与取消重试、双向隧道 |
| 真实浏览器 | 3 项通过，包含真实终端输入、刷新与页面恢复、多页面接管、命令和文件任务 |
| 首段窗口监听 | 连续 361.029 秒，覆盖完整检查及真实 SSH/浏览器运行，控制台窗口显示事件为 0 |
| 打包和安装 | `pnpm dist:win` 通过；Windows PowerShell 5.1 在含空格的隔离路径中安装、认证、停止、卸载通过 |
| 第二段窗口监听 | 连续 417.114 秒，覆盖打包、安装及更新后的本机程序隔离测试，控制台窗口显示事件为 0 |
| 本机已安装程序 | 已备份并更新，SHA-256 与构建产物一致；PE subsystem 为 2；隔离数据目录下 `/health`、停止和干净退出通过 |
| 最终状态 | 正式服务停止，`last-shutdown.json.clean=true`；没有遗留 RemoteDeck 进程、测试 SSH 进程或本次 Docker 夹具 |

生命周期测试最后也改用统一进程 helper，并单独重跑 Windows CLI 测试与
server 严格 Clippy，均通过。Windows 窗口监听使用
`tests/windows/watch_console_windows.py`，需要交互式 Windows 桌面、Python 和
`psutil`。每次传入全新的 output/ready/stop 文件路径，避免旧停止文件使检查
提前结束。监听记录窗口类别和进程身份，不记录终端标题、命令或输出；
零事件描述的是上述实测区间。

安装验收还暴露了 NSIS 临时卸载进程提前返回的竞态。脚本现在等待实际目录
移除后才检查及恢复安装登记，并在失败时保留恢复备份。首次失败后已恢复
本机 RemoteDeck 安装登记；完整复验通过，注册表路径、版本、快捷方式均指向
实际安装，登录启动设置保持关闭。用户配置与专属 `known_hosts` 保留。

当前替代安装包为 `dist/RemoteDeck-2.0.1-win-x64-setup.exe`：
1,381,763 字节，约 1.32 MiB，SHA-256 为
`d8c8e5d77a6a23ff2278a612556a3466e524dc451ac13f5d4cd33feca8e9a04f`，
Authenticode 为 `NotSigned`。本机已安装的 `RemoteDeck.exe` SHA-256 为
`a11c4eac16d7c9fd4770e815a76259434f36a5f2dc4504d5c1e30a66222327ca`。
未推送或发布。

本机证据保存在 `.cache/windowless-verify-final.log`、
`.cache/windowless-openssh-browser.log`、`.cache/windowless-console-events.json`、
`.cache/windowless-package-win.log`、`.cache/windowless-release-acceptance-final.log`、
`.cache/windowless-release-console-events.json`、`.cache/windowless-installed-update.json`、
`.cache/windowless-installed-smoke.json` 和 `.cache/windowless-final-state.json`。
旧程序备份位置记录在 installed-update 文件中。
