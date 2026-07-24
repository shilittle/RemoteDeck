# LabPulse SSH

一个不需要安装额外运行时的 Windows 原生图形工具。程序使用 PowerShell/WPF 与 Windows OpenSSH，复用当前账户的 `ssh lab` 和 `ssh lab-codex` 配置。

## 启动

双击 `Start-LabPulseSSH.cmd`。

如果窗口没有出现，双击 `Start-LabPulseSSH-Debug.cmd` 查看报错。也可以运行自检：

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\LabPulseSSH.ps1 -SelfTest
```

## 功能

- 性能总览：CPU、负载、内存、网络吞吐、磁盘、两张 NVIDIA GPU、运行时间和高 CPU 进程。
- btop 保活：后台维持真正的 btop TTY 会话；btop 退出会重启，每 900 秒主动轮换一次，防止长时间假死。可随时打开独立交互终端。
- 快捷命令：安全只读预设一键执行；自定义命令会按规则判断风险。
- 危险命令：先显示警告，再要求输入指定验证文本。`sudo`、重启、关机、删除、磁盘写入、终止进程、服务停止等都会进入双重验证；需要终端输入的内容在独立 SSH 窗口中处理。
- 端口转发：后台运行 `ssh -NT lab-codex`，定期检查 SSH 服务端点、本地 `127.0.0.1:7890` 和远端 `127.0.0.1:17890`。
- 自动恢复：转发意外退出时，先从服务器端检查并释放专用端口 `17890/tcp`，再按指数退避重连。释放命令先尝试普通 `fuser`，必要时使用工作站允许的 `sudo -n fuser`。

## btop 与图形指标

btop 1.3.0 只有交互 TTY 输出，没有 JSON 或批量导出接口。程序保活真实 btop 会话，但不解析容易损坏的 ANSI 屏幕流。结构化图形面板读取 btop 使用的同源 Linux 数据：

- `/proc/stat`、`/proc/meminfo`、`/proc/net/dev`
- `ps`、文件系统统计
- `nvidia-smi`

`remote_telemetry.py` 每次启动时通过 SSH 标准输入发送，只读运行，不会安装到服务器或修改远端文件。

## 配置

编辑 `config.json` 可以调整：

- SSH 别名；
- 遥测和检查周期；
- btop 主动轮换周期；
- 远程/本地转发端口；
- 命令预设和危险命令正则。

`releaseCommand` 会终止占用专用远端端口 `17890/tcp` 的进程。不要把该端口改成承载其他服务的端口。

## 生命周期与日志

窗口最小化时后台 SSH、btop 与端口转发会继续运行。关闭窗口时，程序会停止自己创建的所有子进程；正常退出会自动释放远端转发监听。

运行日志位于 `logs\monitor-YYYYMMDD.log`。日志不记录 SSH 私钥或服务器密码。
