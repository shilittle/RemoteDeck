[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$InstallerPath,
    [string]$InstallDir,
    [string]$DataDir,
    [string]$LaunchFilePath,
    [string]$ReadyPath = $env:REMOTEDK_READY_PATH,
    [int]$TimeoutSeconds = 60,
    [switch]$KeepInstall
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

foreach ($moduleName in @('Microsoft.PowerShell.Utility', 'Microsoft.PowerShell.Security')) {
    Import-Module (Join-Path $PSHOME "Modules/$moduleName/$moduleName.psd1") -Force
}

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Content
    )
    $encoding = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false
    [IO.File]::WriteAllText($Path, $Content, $encoding)
}

function ConvertTo-ProcessArgument {
    param([AllowEmptyString()][string]$Argument)

    if ($null -eq $Argument -or $Argument.Length -eq 0) { return '""' }
    if ($Argument -notmatch '[\s"]') { return $Argument }

    # ProcessStartInfo.Arguments is the Windows CRT command-line string on
    # Windows PowerShell 5.1. Quote backslashes according to CommandLineToArgvW
    # so isolated paths containing spaces remain one argument.
    $builder = New-Object System.Text.StringBuilder
    [void]$builder.Append('"')
    $backslashes = 0
    foreach ($character in $Argument.ToCharArray()) {
        if ($character -eq '\') {
            $backslashes++
            continue
        }
        if ($character -eq '"') {
            if ($backslashes -gt 0) { [void]$builder.Append((('\' * ($backslashes * 2)) -join '')) }
            [void]$builder.Append('\')
            [void]$builder.Append('"')
            $backslashes = 0
            continue
        }
        if ($backslashes -gt 0) {
            [void]$builder.Append((('\' * $backslashes) -join ''))
            $backslashes = 0
        }
        [void]$builder.Append($character)
    }
    if ($backslashes -gt 0) { [void]$builder.Append((('\' * ($backslashes * 2)) -join '')) }
    [void]$builder.Append('"')
    return $builder.ToString()
}

function Start-HiddenProcess {
    param(
        [Parameter(Mandatory)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$RawArguments,
        [string]$WorkingDirectory,
        [switch]$CaptureOutput
    )

    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $FilePath
    if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
        $startInfo.WorkingDirectory = $WorkingDirectory
    }
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
    if ($CaptureOutput) {
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
    }

    # ArgumentList exists on modern .NET, but not on Windows PowerShell 5.1.
    # Keep the fallback because release verification is required to work there.
    if ($PSBoundParameters.ContainsKey('RawArguments')) {
        # NSIS requires /D=<directory> to be the final raw command-line
        # parameter and explicitly rejects a quoted /D value. Do not route
        # this special case through ArgumentList or the CRT quoting fallback.
        $startInfo.Arguments = $RawArguments
    } else {
        $argumentListProperty = $startInfo.PSObject.Properties['ArgumentList']
        if ($null -ne $argumentListProperty) {
            foreach ($argument in $ArgumentList) { [void]$startInfo.ArgumentList.Add([string]$argument) }
        } elseif ($ArgumentList.Count -gt 0) {
            $startInfo.Arguments = (($ArgumentList | ForEach-Object { ConvertTo-ProcessArgument -Argument ([string]$_) }) -join ' ')
        }
    }

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Could not start hidden process: $FilePath" }
    return $process
}

function Invoke-RegistryTool {
    param([Parameter(Mandatory)][string[]]$RegistryArguments)
    # reg.exe may print even a successful import to stderr. Keep both streams
    # redirected while the hidden child runs; success is determined by exit code.
    $registryProcess = Start-HiddenProcess -FilePath (Join-Path $env:SystemRoot 'System32\reg.exe') -ArgumentList $RegistryArguments -CaptureOutput
    try {
        $null = $registryProcess.StandardOutput.ReadToEnd()
        $null = $registryProcess.StandardError.ReadToEnd()
        $registryProcess.WaitForExit()
        $registryExitCode = $registryProcess.ExitCode
    } finally {
        $registryProcess.Dispose()
    }
    if ($registryExitCode -ne 0) { throw "Registry operation failed with exit code $registryExitCode." }
}

function Save-ShellState {
    param([Parameter(Mandatory)][string]$Root)

    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    $registry = @(
        [pscustomobject]@{ Name = 'install'; Key = 'HKCU\Software\RemoteDeck'; RemoveValue = $null; File = (Join-Path $Root 'install.reg'); Exists = $false },
        [pscustomobject]@{ Name = 'uninstall'; Key = 'HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\RemoteDeck'; RemoveValue = $null; File = (Join-Path $Root 'uninstall.reg'); Exists = $false },
        [pscustomobject]@{ Name = 'run'; Key = 'HKCU\Software\Microsoft\Windows\CurrentVersion\Run'; RemoveValue = 'RemoteDeck'; File = (Join-Path $Root 'run.reg'); Exists = $false }
    )
    foreach ($entry in $registry) {
        $existingKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($entry.Key.Substring(5))
        if ($null -eq $existingKey) { continue }
        $existingKey.Dispose()
        Invoke-RegistryTool -RegistryArguments @('export', $entry.Key, $entry.File, '/y')
        $entry.Exists = $true
    }

    $shortcutPaths = @(
        (Join-Path ([Environment]::GetFolderPath('Desktop')) 'RemoteDeck.lnk'),
        (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\RemoteDeck\RemoteDeck.lnk')
    )
    $shortcuts = @()
    $index = 0
    foreach ($path in $shortcutPaths) {
        $backup = Join-Path $Root "shortcut-$index.lnk"
        $exists = Test-Path -LiteralPath $path -PathType Leaf
        if ($exists) { Copy-Item -LiteralPath $path -Destination $backup -Force }
        $shortcuts += [pscustomobject]@{ Path = $path; Backup = $backup; Exists = $exists }
        $index++
    }
    return [pscustomobject]@{ Registry = $registry; Shortcuts = $shortcuts }
}

function Restore-ShellState {
    param([Parameter(Mandatory)]$State)

    foreach ($entry in $State.Registry) {
        if ($entry.Exists) {
            if (-not $entry.RemoveValue) {
                # These two keys belong exclusively to RemoteDeck. Remove any
                # residue before importing the exact pre-test snapshot.
                [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($entry.Key.Substring(5), $false)
            }
            Invoke-RegistryTool -RegistryArguments @('import', $entry.File)
        } elseif ($entry.RemoveValue) {
            $existingKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($entry.Key.Substring(5), $true)
            if ($null -ne $existingKey) {
                try { $existingKey.DeleteValue($entry.RemoveValue, $false) } finally { $existingKey.Dispose() }
            }
        } else {
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($entry.Key.Substring(5), $false)
        }
    }

    foreach ($shortcut in $State.Shortcuts) {
        if (Test-Path -LiteralPath $shortcut.Path -PathType Leaf) {
            Remove-Item -LiteralPath $shortcut.Path -Force
        }
        if ($shortcut.Exists) {
            $parent = Split-Path -Parent $shortcut.Path
            New-Item -ItemType Directory -Path $parent -Force | Out-Null
            Copy-Item -LiteralPath $shortcut.Backup -Destination $shortcut.Path -Force
        }
    }
}

function Get-ReadyResponse {
    param([Parameter(Mandatory)][string]$BaseUrl, [string]$Path)
    $paths = if ($Path) { @($Path) } else { @('/health', '/api/v1/health', '/api/v1/ready', '/healthz') }
    foreach ($candidate in $paths) {
        $uri = if ($candidate -match '^https?://') { $candidate } else { "$BaseUrl/$($candidate.TrimStart('/'))" }
        try {
            $response = Invoke-WebRequest -Uri $uri -UseBasicParsing -Headers @{} -TimeoutSec 5
            if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 300) { return $response }
        } catch {
            # Keep polling while the service starts. The final error contains the
            # launch-file path and endpoint contract that was observed.
        }
    }
    return $null
}

function Read-LaunchFile {
    param([Parameter(Mandatory)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    try {
        $value = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
        if ($value.baseUrl -is [string] -and $value.baseUrl -match '^http://127\.0\.0\.1:\d+$') {
            return $value
        }
    } catch {
        # The server writes this file atomically; a partial first read is harmless.
    }
    return $null
}

$installer = Get-Item -LiteralPath $InstallerPath -ErrorAction Stop
if ($installer.Extension -ine '.exe') { throw "Installer must be an .exe: $($installer.FullName)" }
if ($installer.Length -ge 40MB) { throw "Installer exceeds the 40 MiB release ceiling: $($installer.Length) bytes." }
$hash = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
$signature = Get-AuthenticodeSignature -LiteralPath $installer.FullName
Write-Host "Installer SHA256: $hash"
Write-Host "Installer Authenticode: $($signature.Status)"

$temporaryInstall = $false
if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:TEMP "RemoteDeck-release-install-$([guid]::NewGuid().ToString('N'))"
    $temporaryInstall = $true
}
if (Test-Path -LiteralPath $InstallDir) { throw "Install directory already exists; refusing to overwrite: $InstallDir" }

$temporaryData = $false
if ([string]::IsNullOrWhiteSpace($DataDir)) {
    $DataDir = Join-Path $env:TEMP "RemoteDeck-release-data-$([guid]::NewGuid().ToString('N'))"
    $temporaryData = $true
}
if (Test-Path -LiteralPath $DataDir) { throw "Data directory already exists; refusing to overwrite: $DataDir" }
New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
$markerPath = Join-Path $DataDir 'release-verification-marker.txt'
Write-Utf8NoBom -Path $markerPath -Content 'RemoteDeck release verification user-data marker'
if ([string]::IsNullOrWhiteSpace($LaunchFilePath)) { $LaunchFilePath = Join-Path $DataDir 'launch.json' }
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$shellStateRoot = Join-Path $env:TEMP "RemoteDeck-release-shell-state-$([guid]::NewGuid().ToString('N'))"
$shellState = Save-ShellState -Root $shellStateRoot
Write-Host "Shell-state backup: $shellStateRoot"
$verificationComplete = $false

$service = $null
try {
    try {
        if ($InstallDir.Contains('"')) { throw 'Install directory cannot contain a double quote.' }
        $installProcess = Start-HiddenProcess -FilePath $installer.FullName -RawArguments ("/S /D=$InstallDir")
        $installProcess.WaitForExit()
        if ($installProcess.ExitCode -ne 0) { throw "Silent install failed with exit code $($installProcess.ExitCode)." }

        $binaryPath = Join-Path $InstallDir 'RemoteDeck.exe'
        if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) { throw "Installed binary is missing: $binaryPath" }

        # --launch-file avoids reading the DPAPI-protected runtime control token.
        # The service writes only its browser URL and loopback base URL to this
        # per-run file; --data-dir keeps the test isolated from user data.
        $service = Start-HiddenProcess -FilePath $binaryPath -WorkingDirectory $InstallDir -ArgumentList @('--no-open', '--data-dir', $DataDir, '--launch-file', $LaunchFilePath)
        $launch = $null
        $endpoint = $null
        $ready = $null
        $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
        while ((Get-Date) -lt $deadline) {
            $service.Refresh()
            if ($service.HasExited) { throw "Installed RemoteDeck.exe exited before the loopback service became ready (code $($service.ExitCode))." }
            $launch = Read-LaunchFile -Path $LaunchFilePath
            if ($launch) {
                $endpoint = $launch.baseUrl.TrimEnd('/')
                $ready = Get-ReadyResponse -BaseUrl $endpoint -Path $ReadyPath
                if ($ready) { break }
            }
            Start-Sleep -Milliseconds 250
        }
        if (-not $ready -or -not $endpoint) {
            throw "Loopback service did not expose a ready API within $TimeoutSeconds seconds. Launch file: $LaunchFilePath."
        }

        $running = Get-Process -Id $service.Id -ErrorAction SilentlyContinue
        if (-not $running) { throw "The service process $($service.Id) was not independently observable after readiness." }
        $service.Refresh()
        if ($service.HasExited) { throw 'RemoteDeck exited immediately after the ready API response.' }
        Write-Host "Loopback service ready: $endpoint"
        Write-Host "Independent process check passed: PID $($service.Id) remains running."

        $webPage = Invoke-WebRequest -Uri "$endpoint/" -UseBasicParsing -TimeoutSec 5
        if ($webPage.Content -notmatch '/assets/[^" ]+\.js') { throw 'Installed server did not serve the embedded WebUI entry.' }
        $ticket = ([uri]$launch.browserUrl).Fragment -replace '^#ticket=', ''
        if (-not $ticket) { throw 'Launch file did not contain a browser ticket.' }
        $browserSession = New-Object Microsoft.PowerShell.Commands.WebRequestSession
        $auth = Invoke-RestMethod -Uri "$endpoint/api/v1/auth/exchange" -Method Post -ContentType 'application/json' -Headers @{ Origin = $endpoint } -WebSession $browserSession -Body (@{ ticket = $ticket } | ConvertTo-Json) -TimeoutSec 5
        $snapshot = Invoke-RestMethod -Uri "$endpoint/api/v1/bootstrap" -Method Post -ContentType 'application/json' -Headers @{ Origin = $endpoint; 'X-RemoteDeck-CSRF' = $auth.csrfToken } -WebSession $browserSession -Body '{}' -TimeoutSec 5
        if (-not $snapshot.appVersion) { throw 'Installed service did not return an authenticated application snapshot.' }
        Write-Host 'Embedded WebUI, browser ticket exchange and authenticated bootstrap passed.'

        $stopper = Start-HiddenProcess -FilePath $binaryPath -WorkingDirectory $InstallDir -ArgumentList @('--stop', '--data-dir', $DataDir)
        $stopper.WaitForExit()
        if ($stopper.ExitCode -ne 0) { throw "The service --stop command failed with exit code $($stopper.ExitCode)." }
        if (-not $service.WaitForExit(10000)) { throw 'The service did not exit after the explicit --stop command.' }
        $shutdown = Get-Content -LiteralPath (Join-Path $DataDir 'last-shutdown.json') -Raw | ConvertFrom-Json
        if (-not $shutdown.clean) { throw 'Installed service reported incomplete resource cleanup.' }
        Write-Host 'Owned service stopped through the production --stop command.'
    }
    finally {
        if ($service) {
            $service.Refresh()
            if (-not $service.HasExited) {
                # Cleanup is limited to the process started by this script. A failed
                # readiness check or --stop path must not leave a test service behind.
                $service.Kill()
                $service.WaitForExit(10000)
            }
        }
    }

    if (-not $KeepInstall) {
        $uninstallerPath = Join-Path $InstallDir 'uninstall.exe'
        if (-not (Test-Path -LiteralPath $uninstallerPath -PathType Leaf)) { throw "Uninstaller is missing: $uninstallerPath" }
        $uninstallProcess = Start-HiddenProcess -FilePath $uninstallerPath -ArgumentList @('/S')
        $uninstallProcess.WaitForExit()
        if ($uninstallProcess.ExitCode -ne 0) { throw "Silent uninstall failed with exit code $($uninstallProcess.ExitCode)." }
        # NSIS first starts a temporary copy of its uninstaller and exits. Its
        # first process exiting does not mean that copy has removed the files
        # and shared registry keys. Wait before inspecting or restoring them.
        $uninstallDeadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        while ((Test-Path -LiteralPath $InstallDir) -and [DateTime]::UtcNow -lt $uninstallDeadline) {
            Start-Sleep -Milliseconds 100
        }
        if (Test-Path -LiteralPath $InstallDir) { throw "Install directory remained after uninstall: $InstallDir" }
        $runValue = Get-ItemProperty -Path $runKey -Name RemoteDeck -ErrorAction SilentlyContinue
        if ($null -ne $runValue) { throw 'The RemoteDeck startup Run value remained after uninstall.' }
        if (-not (Test-Path -LiteralPath $DataDir) -or -not (Test-Path -LiteralPath $markerPath -PathType Leaf)) {
            throw 'Uninstall removed the isolated user-data directory or its marker.'
        }
        Write-Host 'Clean current-user uninstall passed; isolated user data was left outside the install directory.'
    }
    else {
        Write-Host "Keeping install and isolated data for inspection: $InstallDir / $DataDir"
    }
    $verificationComplete = $true
}
finally {
    if (-not $KeepInstall) {
        # NSIS uses fixed current-user registry keys and shortcut names. Restore
        # their exact pre-test state so an isolated smoke cannot disrupt an
        # existing RemoteDeck installation in this profile.
        Restore-ShellState -State $shellState
    }
    if (-not $verificationComplete) {
        Write-Host "Verification failed; retaining shell-state backup for recovery: $shellStateRoot"
    }
    if ($verificationComplete -and (Test-Path -LiteralPath $shellStateRoot)) {
        $resolvedStateRoot = [IO.Path]::GetFullPath($shellStateRoot)
        $tempPrefix = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
        if (-not $resolvedStateRoot.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolvedStateRoot) -notmatch '^RemoteDeck-release-shell-state-[a-f0-9]{32}$') {
            throw 'Refusing to remove a shell-state backup outside the dedicated temporary directory.'
        }
        Remove-Item -LiteralPath $shellStateRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
