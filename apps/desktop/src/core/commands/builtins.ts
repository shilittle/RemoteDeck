import type { CommandPreset } from '../../protocol/domain'

type Builtin = CommandPreset & { builtin: true; dependency?: string }
const createdAt = '2026-01-01T00:00:00.000Z'

function builtin(id: string, name: string, description: string, group: string, command: string, risk: CommandPreset['risk'], sortOrder: number, options: Partial<Pick<CommandPreset, 'requiresPty' | 'requiresSudo' | 'confirmationText'>> & { dependency?: string } = {}): Builtin {
  return {
    schemaVersion: 1, id, name, description, group, command, risk, sortOrder, createdAt, updatedAt: createdAt, builtin: true,
    requiresPty: options.requiresPty ?? false, requiresSudo: options.requiresSudo ?? false,
    ...(options.confirmationText ? { confirmationText: options.confirmationText } : {}),
    ...(options.dependency ? { dependency: options.dependency } : {})
  }
}

export const builtinCommands: Builtin[] = [
  builtin('10000000-0000-4000-8000-000000000001', '系统概览', '内核、主机、身份和负载信息', '系统', 'uname -a; hostname; whoami; uptime; lscpu | head -n 20', 'L0', 10),
  builtin('10000000-0000-4000-8000-000000000002', 'GPU 状态', 'NVIDIA GPU 与进程状态', '资源', 'nvidia-smi', 'L0', 20, { dependency: 'nvidia-smi' }),
  builtin('10000000-0000-4000-8000-000000000003', 'CPU / 内存进程', '按 CPU 排序的前 30 个进程', '资源', 'ps -eo pid,ppid,user,pcpu,pmem,state,etime,args --sort=-pcpu | head -n 31', 'L0', 30),
  builtin('10000000-0000-4000-8000-000000000004', '磁盘与 inode', '所有挂载点的容量与 inode 使用量', '资源', 'df -hT; printf "\\n"; df -ih', 'L0', 40),
  builtin('10000000-0000-4000-8000-000000000005', '当前用户任务', '仅列出当前登录用户的进程', '系统', 'ps -u "$USER" -o pid,ppid,pcpu,pmem,state,etime,args --sort=-pcpu', 'L0', 50),
  builtin('10000000-0000-4000-8000-000000000006', '监听端口', '列出 TCP/UDP 监听端点', '网络', 'ss -lntup', 'L0', 60, { dependency: 'ss' }),
  builtin('10000000-0000-4000-8000-000000000007', 'Git 状态', '当前关联工作区的 Git 状态', '开发', 'git status --short --branch', 'L0', 70, { dependency: 'git' }),
  builtin('10000000-0000-4000-8000-000000000008', 'Slurm 队列', '当前用户的 Slurm 作业', '调度', 'squeue -u "$USER"', 'L0', 80, { dependency: 'squeue' }),
  builtin('10000000-0000-4000-8000-000000000009', 'btop', '在真实 PTY 中打开 btop', '资源', 'exec btop', 'L0', 90, { requiresPty: true, dependency: 'btop' }),
  builtin('10000000-0000-4000-8000-000000000010', '重启工作站', '通过 systemd 重启远端 Linux 主机', '高风险', 'systemctl reboot', 'L2', 100, { requiresPty: true, requiresSudo: true })
]

export function getBuiltin(id: string): Builtin | undefined { return builtinCommands.find((item) => item.id === id) }
