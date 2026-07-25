# Windows code signing / Windows 代码签名

[简体中文](#简体中文) · [English](#english)

## 简体中文

### 信任模型

RemoteDeck 的正式签名构建使用 Authenticode。签名必须满足：

- 使用受公众信任的 Windows 代码签名证书，不能用自签名证书冒充正式发布者。
- electron-builder 必须启用 `forceCodeSigning`；没有可用签名身份时构建直接失败。
- 打包后的 `RemoteDeck.exe`、NSIS 安装器和 portable EXE 的 `Get-AuthenticodeSignature` 状态都必须为 `Valid`。
- 三个 EXE 都必须包含 RFC 3161 时间戳。
- Release 说明必须列出发布者和 SHA-256；上传后必须重新下载并复验。

常规 OV 证书能确认发布者身份，但 SmartScreen 声誉仍需要逐步积累。EV 或 Azure Trusted Signing 通常能更快建立信任，但需要硬件令牌或外部 Azure 账户。RemoteDeck 不会把证书、私钥或口令提交到 Git。

### 本地签名

准备受信任 CA 签发、包含私钥的 `.pfx`/`.p12`：

```powershell
$env:WIN_CSC_LINK = 'C:\secure\RemoteDeck-code-signing.pfx'
$env:WIN_CSC_KEY_PASSWORD = Read-Host 'PFX password'
$env:REMOTEDECK_SIGNER_SUBJECT = '证书 Subject 中应出现的发布者'

pnpm install --frozen-lockfile
pnpm verify
pnpm test:e2e
pnpm dist:win:signed
pnpm verify:signatures
```

不要把证书放入仓库目录，也不要把口令写入脚本、命令参数、`.env`、日志或发布说明。工作完成后清除环境变量：

```powershell
Remove-Item Env:WIN_CSC_LINK,Env:WIN_CSC_KEY_PASSWORD,Env:REMOTEDECK_SIGNER_SUBJECT -ErrorAction SilentlyContinue
```

### GitHub Actions 签名

工作流 [`.github/workflows/signed-windows.yml`](../.github/workflows/signed-windows.yml) 只允许有写权限的维护者手动触发，并在 `release` environment 中读取：

- Secret `WIN_CSC_LINK`：Base64 编码的 `.pfx/.p12`，或 electron-builder 支持的安全 URL。
- Secret `WIN_CSC_KEY_PASSWORD`：证书口令。
- Variable `REMOTEDECK_SIGNER_SUBJECT`：预期发布者 Subject 的稳定片段。

建议在 `release` environment 上启用 required reviewer。工作流不自动创建 Release；它只生成已签名且完成测试、安装冒烟、签名验证和 SHA-256 清单的短期 artifact。维护者下载并复验后，再显式发布。

### 发布验收

```powershell
Get-ChildItem .\release\*.exe | ForEach-Object {
  Get-AuthenticodeSignature -LiteralPath $_.FullName |
    Select-Object Path,Status,SignerCertificate,TimeStamperCertificate
}
Get-ChildItem .\release\RemoteDeck-* | Get-FileHash -Algorithm SHA256
```

若任一签名不是 `Valid`、发布者不一致、没有时间戳、哈希不一致或证书已被吊销，不得上传或描述为已签名版本。

## English

### Trust model

Official signed RemoteDeck builds use Authenticode and must satisfy every condition below:

- Use a publicly trusted Windows code-signing certificate. A self-signed certificate must never be presented as a production publisher identity.
- Enable electron-builder `forceCodeSigning`; the build must fail when no signing identity is available.
- `Get-AuthenticodeSignature` must report `Valid` for packaged `RemoteDeck.exe`, the NSIS installer, and the portable EXE.
- All three executables must carry an RFC 3161 timestamp.
- Release notes must state the publisher and SHA-256 values. Uploaded assets must be downloaded and verified again.

A standard OV certificate identifies the publisher, but SmartScreen reputation can still take time to build. EV certificates or Azure Trusted Signing usually establish trust faster but require a hardware token or an external Azure account. RemoteDeck never commits certificates, private keys, or passwords to Git.

### Local signing

Prepare a trusted CA-issued `.pfx`/`.p12` that includes the private key:

```powershell
$env:WIN_CSC_LINK = 'C:\secure\RemoteDeck-code-signing.pfx'
$env:WIN_CSC_KEY_PASSWORD = Read-Host 'PFX password'
$env:REMOTEDECK_SIGNER_SUBJECT = 'stable publisher fragment from the certificate Subject'

pnpm install --frozen-lockfile
pnpm verify
pnpm test:e2e
pnpm dist:win:signed
pnpm verify:signatures
```

Keep the certificate outside the repository. Never place its password in a script, command argument, `.env` file, log, or Release notes. Clear the environment after signing:

```powershell
Remove-Item Env:WIN_CSC_LINK,Env:WIN_CSC_KEY_PASSWORD,Env:REMOTEDECK_SIGNER_SUBJECT -ErrorAction SilentlyContinue
```

### GitHub Actions signing

The [`.github/workflows/signed-windows.yml`](../.github/workflows/signed-windows.yml) workflow is manually dispatched by a maintainer with write access. Its `release` environment reads:

- `WIN_CSC_LINK` secret: Base64-encoded `.pfx/.p12` content or another secure URL accepted by electron-builder.
- `WIN_CSC_KEY_PASSWORD` secret: certificate password.
- `REMOTEDECK_SIGNER_SUBJECT` variable: a stable fragment of the expected publisher Subject.

Enable required reviewers on the `release` environment where possible. The workflow does not create a Release automatically. It produces a short-lived artifact only after tests, packaging smokes, signature verification, and SHA-256 manifest generation pass. A maintainer downloads and verifies it before an explicit publication action.

### Release acceptance

```powershell
Get-ChildItem .\release\*.exe | ForEach-Object {
  Get-AuthenticodeSignature -LiteralPath $_.FullName |
    Select-Object Path,Status,SignerCertificate,TimeStamperCertificate
}
Get-ChildItem .\release\RemoteDeck-* | Get-FileHash -Algorithm SHA256
```

Do not upload or describe a build as signed if any signature is not `Valid`, the publisher differs, the timestamp is absent, a hash differs, or the certificate has been revoked.
