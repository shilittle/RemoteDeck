[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[A-Za-z0-9_.-]+$')]
  [string]$HostAlias,

  [string]$Workspace = '~',

  [switch]$RunDeviceLogin
)

$ErrorActionPreference = 'Stop'

function Quote-Remote([string]$Value) {
  $quote = [string][char]39
  $replacement = $quote + '"' + $quote + '"' + $quote
  return $quote + $Value.Replace($quote, $replacement) + $quote
}

$workspaceArgument = Quote-Remote $Workspace
$probe = @'
printf 'codex='; command -v codex || true; codex --version 2>/dev/null || true; codex login status 2>&1 || true; printf 'tmux='; command -v tmux || true; cd -- __WORKSPACE__ 2>/dev/null && printf 'cwd=%s\n' "$PWD" && git status --short --branch 2>/dev/null || true
'@
$probe = $probe.Replace('__WORKSPACE__', $workspaceArgument).Trim()

Write-Host "M7 real-host probe: $HostAlias"
& ssh -- $HostAlias $probe
if ($LASTEXITCODE -ne 0) { throw "SSH probe failed with exit code $LASTEXITCODE" }

Write-Host ''
Write-Host 'In RemoteDeck, verify:'
Write-Host '  1. L0 system overview runs directly and produces real output.'
Write-Host '  2. L1 shows the full command, alias, and workspace before execution.'
Write-Host '  3. L2 rejects incorrect text and accepts only the alias or configured phrase.'
Write-Host '  4. Cancel one long task while a second task and SSH terminal continue.'
Write-Host '  5. Codex status matches the probe; start/resume opens a genuine PTY.'
Write-Host '  6. Persistent start and reattach use the same remotedeck-* tmux session.'

if ($RunDeviceLogin) {
  Write-Host ''
  Write-Host 'Starting explicit device-code login. No credential file will be read or copied.'
  & ssh -t -- $HostAlias "cd -- $workspaceArgument && exec codex login --device-auth"
  if ($LASTEXITCODE -ne 0) { throw "Device login exited with code $LASTEXITCODE" }
}
