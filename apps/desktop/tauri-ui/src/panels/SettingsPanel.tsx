import { useEffect, useState } from "react";
import {
  FileArchive,
  FolderOpen,
  RotateCcw,
  Save,
  Settings,
} from "lucide-react";
import { api, errorMessage } from "../api";
import { useAppStore } from "../store";
import type { AppSettings } from "../types";
import { MigrationPanel } from "./MigrationPanel";

export function SettingsPanel(): React.JSX.Element {
  const settings = useAppStore((state) => state.settings);
  const updateSettings = useAppStore((state) => state.updateSettings);
  const capabilities = useAppStore((state) => state.capabilities);
  const busy = useAppStore((state) => state.busy);
  const [draft, setDraft] = useState<AppSettings>(settings);
  const [diagnosticsBusy, setDiagnosticsBusy] = useState(false);
  const [directoryPickerBusy, setDirectoryPickerBusy] = useState(false);
  const [diagnosticsMessage, setDiagnosticsMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => setDraft(settings), [settings]);
  const patch = <K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K],
  ): void => setDraft((current) => ({ ...current, [key]: value }));
  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);

  const save = async (): Promise<void> => {
    setError("");
    try {
      await updateSettings(draft);
    } catch (reason) {
      setError(errorMessage(reason));
    }
  };

  const exportDiagnostics = async (): Promise<void> => {
    setDiagnosticsBusy(true);
    setDiagnosticsMessage("");
    setError("");
    try {
      const result = await api.exportDiagnostics();
      setDiagnosticsMessage(
        result.exported
          ? `已导出 ${String(result.entries)} 项 · SHA-256 ${result.sha256 ?? "未知"}\n${result.path ?? ""}`
          : "已取消导出。",
      );
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setDiagnosticsBusy(false);
    }
  };

  const pickDownloadDirectory = async (): Promise<void> => {
    setDirectoryPickerBusy(true);
    setError("");
    try {
      const selected = await api.pickLocalPath(true);
      if (selected) patch("downloadDirectory", selected);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setDirectoryPickerBusy(false);
    }
  };

  return (
    <section className="panel settings-panel">
      <header className="panel-heading">
        <div>
          <h1>设置</h1>
          <p>所有设置通过版本化原子存储保存，敏感凭据不会进入配置或诊断包。</p>
        </div>
        <div className="button-row wrap">
          <button
            disabled={diagnosticsBusy}
            onClick={() => {
              void exportDiagnostics();
            }}
          >
            <FileArchive size={15} />
            {diagnosticsBusy ? "导出中…" : "导出诊断包"}
          </button>
          <button disabled={!dirty} onClick={() => setDraft(settings)}>
            <RotateCcw size={15} />
            撤销
          </button>
          <button
            className="primary"
            disabled={busy || !dirty}
            onClick={() => {
              void save();
            }}
          >
            <Save size={15} />
            保存
          </button>
        </div>
      </header>
      {error && (
        <div className="notice error" role="alert">
          {error}
        </div>
      )}
      {diagnosticsMessage && (
        <pre className="diagnostics-result" role="status">
          {diagnosticsMessage}
        </pre>
      )}
      <div className="settings-grid">
        <section className="card">
          <div className="card-title">
            <Settings size={16} />
            <h2>终端与连接</h2>
          </div>
          <label>
            <span>终端字体</span>
            <input
              value={draft.terminalFontFamily}
              onChange={(event) =>
                patch("terminalFontFamily", event.target.value)
              }
            />
          </label>
          <label>
            <span>终端字号</span>
            <input
              type="number"
              min={9}
              max={32}
              value={draft.terminalFontSize}
              onChange={(event) =>
                patch("terminalFontSize", Number(event.target.value))
              }
            />
          </label>
          <label className="toggle">
            <input
              type="checkbox"
              checked={draft.autoReconnect}
              onChange={(event) => patch("autoReconnect", event.target.checked)}
            />
            <span>遥测连接异常后恢复本应用采集器</span>
          </label>
          <label>
            <span>默认 AI Agent</span>
            <select
              value={draft.defaultAgent}
              onChange={(event) =>
                patch(
                  "defaultAgent",
                  event.target.value as AppSettings["defaultAgent"],
                )
              }
            >
              <option value="codex">Codex CLI</option>
              <option value="claude">Claude Code</option>
              <option value="gemini">Gemini CLI</option>
              <option value="opencode">OpenCode</option>
            </select>
          </label>
          <label>
            <span>默认下载目录</span>
            <div className="settings-path-row">
              <input
                value={draft.downloadDirectory}
                onChange={(event) =>
                  patch("downloadDirectory", event.target.value)
                }
              />
              <button
                type="button"
                disabled={directoryPickerBusy}
                onClick={() => {
                  void pickDownloadDirectory();
                }}
              >
                <FolderOpen size={14} />
                {directoryPickerBusy ? "选择中…" : "浏览"}
              </button>
            </div>
          </label>
        </section>
        <section className="card">
          <div className="card-title">
            <Settings size={16} />
            <h2>监控与运行</h2>
          </div>
          <label>
            <span>采样间隔（秒）</span>
            <input
              type="number"
              min={1}
              max={60}
              value={draft.telemetryIntervalSeconds}
              onChange={(event) =>
                patch("telemetryIntervalSeconds", Number(event.target.value))
              }
            />
          </label>
          <label>
            <span>历史保留（分钟）</span>
            <input
              type="number"
              min={1}
              max={1440}
              value={draft.telemetryRetentionMinutes}
              onChange={(event) =>
                patch("telemetryRetentionMinutes", Number(event.target.value))
              }
            />
          </label>
          <label className="toggle">
            <input
              type="checkbox"
              checked={draft.btopWatchdogEnabled}
              onChange={(event) =>
                patch("btopWatchdogEnabled", event.target.checked)
              }
            />
            <span>随启动时自动监控启用 btop watchdog</span>
          </label>
          <label>
            <span>btop 轮换（分钟）</span>
            <input
              type="number"
              min={1}
              max={1440}
              value={draft.btopRotationMinutes}
              onChange={(event) =>
                patch("btopRotationMinutes", Number(event.target.value))
              }
            />
          </label>
          <label>
            <span>日志级别</span>
            <select
              value={draft.logLevel}
              onChange={(event) =>
                patch("logLevel", event.target.value as AppSettings["logLevel"])
              }
            >
              <option value="debug">Debug</option>
              <option value="info">Info</option>
              <option value="warn">Warn</option>
              <option value="error">Error</option>
            </select>
          </label>
        </section>
        <section className="card">
          <div className="card-title">
            <Settings size={16} />
            <h2>Windows 集成</h2>
          </div>
          <label className="toggle">
            <input
              type="checkbox"
              checked={draft.closeToTray}
              onChange={(event) => patch("closeToTray", event.target.checked)}
            />
            <span>关闭窗口时保留到托盘</span>
          </label>
          <label className="toggle">
            <input
              type="checkbox"
              checked={draft.launchAtLogin}
              onChange={(event) => patch("launchAtLogin", event.target.checked)}
            />
            <span>登录 Windows 后启动到托盘</span>
          </label>
          <button onClick={() => patch("onboardingCompleted", false)}>
            下次启动重新显示首次向导
          </button>
          <dl className="capabilities">
            <dt>OpenSSH</dt>
            <dd>{capabilities?.sshPath ?? "未发现"}</dd>
            <dt>ConPTY</dt>
            <dd>{capabilities?.pty ? "可用" : "不可用"}</dd>
            <dt>SFTP</dt>
            <dd>{capabilities?.sftp === false ? "不可用" : "可用"}</dd>
            <dt>遥测</dt>
            <dd>{capabilities?.telemetry === false ? "不可用" : "可用"}</dd>
          </dl>
        </section>
      </div>
      <MigrationPanel />
      <div className="architecture-note">
        <strong>轻量发行边界</strong>
        <p>
          仅构建 Tauri 2 NSIS；使用系统 WebView2 与 Windows OpenSSH，不内嵌
          Electron、Chromium、Node.js 或 Python 运行时。
        </p>
      </div>
    </section>
  );
}
