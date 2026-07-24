param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,
    [int]$StartupTimeoutSeconds = 20
)

$ErrorActionPreference = 'Stop'
$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
$tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$installRoot = Join-Path $tempRoot ("RemoteDeck-clean-install-" + [guid]::NewGuid().ToString('N'))
$process = $null
try {
    $installer = Start-Process -FilePath $resolvedInstaller -ArgumentList @('/S', "/D=$installRoot") -PassThru -Wait
    if ($installer.ExitCode -ne 0) { throw "Installer returned exit code $($installer.ExitCode)." }
    $executable = Join-Path $installRoot 'RemoteDeck.exe'
    if (-not (Test-Path -LiteralPath $executable)) { throw "Installed executable is missing: $executable" }
    $userData = Join-Path $installRoot 'acceptance-user-data'
    $process = Start-Process -FilePath $executable -ArgumentList @("--user-data-dir=$userData", '--disable-gpu') -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 500
        $process.Refresh()
        if ($process.HasExited) { throw "Installed application exited during startup with code $($process.ExitCode)." }
    } while ([DateTime]::UtcNow -lt $deadline -and [string]::IsNullOrWhiteSpace($process.MainWindowTitle))
    if ($process.MainWindowTitle -notlike '*RemoteDeck*') { throw 'Installed application did not create the expected window.' }
    Write-Host "PASS clean install and launch: $installRoot"
} finally {
    $owned = Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'RemoteDeck.exe' -and $null -ne $_.ExecutablePath -and $_.ExecutablePath.StartsWith($installRoot, [StringComparison]::OrdinalIgnoreCase) }
    if ($null -ne $owned) { Stop-Process -Id @($owned.ProcessId) -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 2
    $uninstaller = Join-Path $installRoot 'Uninstall RemoteDeck.exe'
    if (Test-Path -LiteralPath $uninstaller) {
        $uninstall = Start-Process -FilePath $uninstaller -ArgumentList '/S' -PassThru -Wait
        if ($uninstall.ExitCode -ne 0) { throw "Uninstaller returned exit code $($uninstall.ExitCode)." }
    }
    $resolvedInstall = [IO.Path]::GetFullPath($installRoot)
    if ($resolvedInstall.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolvedInstall)) {
        Remove-Item -LiteralPath $resolvedInstall -Recurse -Force
    }
}
