[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })]
  [string]$RemoteDeckExecutable
)

$ErrorActionPreference = 'Stop'
$resolvedExecutable = (Resolve-Path -LiteralPath $RemoteDeckExecutable).Path
$process = Start-Process -FilePath $resolvedExecutable -PassThru

Write-Host 'M8 native lifecycle acceptance'
Write-Host '  1. Connect a test Linux host and start one tunnel.'
Write-Host '  2. Close the RemoteDeck window. Confirm the tray icon remains.'
Write-Host '  3. Confirm the tunnel still passes traffic while the window is hidden.'
Write-Host '  4. Restore from the tray and confirm SSH/terminal/task state remains.'
Write-Host '  5. Suspend and resume Windows; confirm the tunnel and monitoring recover.'
Write-Host '  6. Use the tray “Fully Quit” action and confirm tunnel traffic stops.'
Read-Host 'Press Enter after completing the checks'

$process.Refresh()
if (-not $process.HasExited) {
  throw 'RemoteDeck is still running. Complete the Fully Quit check before accepting M8.'
}
Write-Host 'M8 tray lifecycle acceptance completed.'
