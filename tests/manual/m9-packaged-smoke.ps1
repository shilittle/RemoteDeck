param(
    [Parameter(Mandatory = $true)]
    [string]$PortablePath,
    [int]$StartupTimeoutSeconds = 20
)

$ErrorActionPreference = 'Stop'
$resolvedPortable = (Resolve-Path -LiteralPath $PortablePath).Path
if ([IO.Path]::GetExtension($resolvedPortable) -ne '.exe') { throw 'PortablePath must point to an .exe artifact.' }
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ("RemoteDeck-portable-smoke-" + [guid]::NewGuid().ToString('N'))
[IO.Directory]::CreateDirectory($testRoot) | Out-Null
$launcher = $null
try {
    $launcher = Start-Process -FilePath $resolvedPortable -ArgumentList @("--user-data-dir=$testRoot", '--disable-gpu') -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    $windowProcess = $null
    do {
        Start-Sleep -Milliseconds 500
        $candidate = Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'RemoteDeck.exe' -and $null -ne $_.CommandLine -and $_.CommandLine.Contains($testRoot) } | Select-Object -First 1
        if ($null -ne $candidate) {
            $windowProcess = Get-Process -Id $candidate.ProcessId -ErrorAction SilentlyContinue
            if ($null -ne $windowProcess) { $windowProcess.Refresh() }
        }
    } while ([DateTime]::UtcNow -lt $deadline -and ($null -eq $windowProcess -or [string]::IsNullOrWhiteSpace($windowProcess.MainWindowTitle)))
    if ($null -eq $windowProcess -or [string]::IsNullOrWhiteSpace($windowProcess.MainWindowTitle)) { throw 'Portable application did not create a window before the timeout.' }
    if ($windowProcess.MainWindowTitle -notlike '*RemoteDeck*') { throw "Unexpected window title: $($windowProcess.MainWindowTitle)" }
    Write-Host "PASS portable launch: $resolvedPortable"
    Write-Host "PASS isolated userData: $testRoot"
} finally {
    $owned = Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'RemoteDeck.exe' -and $null -ne $_.CommandLine -and $_.CommandLine.Contains($testRoot) }
    if ($null -ne $owned) { Stop-Process -Id @($owned.ProcessId) -Force -ErrorAction SilentlyContinue }
    if ($null -ne $launcher -and -not $launcher.HasExited) { Stop-Process -Id $launcher.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Seconds 2
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedRoot.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $resolvedRoot)) {
        Remove-Item -LiteralPath $resolvedRoot -Recurse -Force -ErrorAction Stop
    }
}
