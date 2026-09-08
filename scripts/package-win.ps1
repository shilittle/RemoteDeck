[CmdletBinding()]
param(
    [string]$Root,
    [string]$Version,
    [string]$TargetDir
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Package managers launched by PowerShell 7 can inherit its module search path
# into Windows PowerShell 5.1. Load the matching built-in modules explicitly.
foreach ($moduleName in @('Microsoft.PowerShell.Utility', 'Microsoft.PowerShell.Security')) {
    Import-Module (Join-Path $PSHOME "Modules/$moduleName/$moduleName.psd1") -Force
}

if ([string]::IsNullOrWhiteSpace($Root)) {
    $Root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
}

function Resolve-FirstExistingPath {
    param([Parameter(Mandatory)][string[]]$Candidates)
    foreach ($candidate in $Candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) { continue }
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    return $null
}

function Convert-ToNsisPath {
    param([Parameter(Mandatory)][string]$Path)
    # NSIS accepts native Windows paths. Keep the drive prefix and backslashes;
    # converting an absolute path to a slash path makes File resolve it as a
    # relative wildcard on some cached makensis builds.
    return $Path
}

Set-Location -LiteralPath $Root

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Content
    )
    $encoding = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false
    [IO.File]::WriteAllText($Path, $Content, $encoding)
}

$packageJsonPath = Join-Path $Root 'package.json'
if (-not (Test-Path -LiteralPath $packageJsonPath -PathType Leaf)) {
    throw "Root package.json was not found: $packageJsonPath"
}
$package = Get-Content -LiteralPath $packageJsonPath -Raw | ConvertFrom-Json
if ([string]::IsNullOrWhiteSpace($Version)) { $Version = [string]$package.version }
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$') {
    throw "Invalid release version: $Version"
}

$webDist = Join-Path $Root 'apps/web/dist'
$webPackage = Join-Path $Root 'apps/web/package.json'
if (-not (Test-Path -LiteralPath $webPackage -PathType Leaf)) {
    throw "WebUI package manifest is missing: $webPackage"
}
Write-Host 'Building the WebUI before compiling the embedded server assets.'
& pnpm --filter @remotedeck/web build
if ($LASTEXITCODE -ne 0) { throw "WebUI build failed with exit code $LASTEXITCODE." }
$webEntry = Join-Path $webDist 'index.html'
if (-not (Test-Path -LiteralPath $webEntry -PathType Leaf)) {
    throw "WebUI build output is missing. Run 'pnpm build' first: $webEntry"
}

$cargoManifest = Join-Path $Root 'Cargo.toml'
if (-not (Test-Path -LiteralPath $cargoManifest -PathType Leaf)) {
    throw "Cargo workspace manifest is missing: $cargoManifest"
}

$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$releaseDir = Join-Path $targetRoot 'release'
Write-Host "Building remotedeck-server from $cargoManifest"
& cargo build --manifest-path $cargoManifest --release -p remotedeck-server
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE." }

$binary = Resolve-FirstExistingPath -Candidates @(
    (Join-Path $releaseDir 'RemoteDeck.exe'),
    (Join-Path $releaseDir 'remotedeck-server.exe')
)
if (-not $binary) {
    throw "The release binary was not found in $releaseDir. Expected RemoteDeck.exe from package remotedeck-server."
}
if ([IO.Path]::GetFileName($binary) -cne 'RemoteDeck.exe') {
    throw "The server package produced '$([IO.Path]::GetFileName($binary))'. Configure the binary name as RemoteDeck.exe; the packaging script will not silently rename it."
}

$nsi = Join-Path $Root 'scripts/installer.nsi'
if (-not (Test-Path -LiteralPath $nsi -PathType Leaf)) { throw "NSIS script is missing: $nsi" }
$icon = Join-Path $Root 'apps/server/icons/icon.ico'
if (-not (Test-Path -LiteralPath $icon -PathType Leaf)) { throw "Windows installer icon is missing: $icon" }

$programFiles = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) | Where-Object { $_ }
$tauriCacheRoots = @(
    (Join-Path $env:LOCALAPPDATA 'tauri'),
    (Join-Path $env:APPDATA 'tauri'),
    (Join-Path $env:USERPROFILE '.cache/tauri'),
    (Join-Path $env:USERPROFILE '.cargo/bin')
)
$makensisCandidates = @()
$makensisCommand = Get-Command makensis.exe -ErrorAction SilentlyContinue
if ($makensisCommand) { $makensisCandidates += $makensisCommand.Source }
foreach ($base in $programFiles) {
    $makensisCandidates += (Join-Path $base 'NSIS/makensis.exe')
    $makensisCandidates += (Join-Path $base 'NSIS/Bin/makensis.exe')
}
foreach ($base in $tauriCacheRoots) {
    $makensisCandidates += (Join-Path $base 'NSIS/makensis.exe')
    $makensisCandidates += (Join-Path $base 'NSIS/Bin/makensis.exe')
    $makensisCandidates += (Join-Path $base 'NSIS/nsis-3.10/makensis.exe')
    $makensisCandidates += (Join-Path $base 'NSIS/nsis-3.11/makensis.exe')
    $makensisCandidates += (Join-Path $base 'NSIS/nsis-3.12/makensis.exe')
}
$makensis = Resolve-FirstExistingPath -Candidates ($makensisCandidates | Select-Object -Unique)
if (-not $makensis) {
    throw "makensis.exe was not found. Install NSIS or make the existing Tauri NSIS cache available; this script does not install tauri-cli."
}

$outputDir = if ($TargetDir) { [IO.Path]::GetFullPath($TargetDir) } else { Join-Path $Root 'dist' }
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
Get-ChildItem -LiteralPath $outputDir -Filter 'RemoteDeck-*-win-x64-setup.exe' -File -ErrorAction SilentlyContinue | Remove-Item -Force
foreach ($stale in @('SHA256SUMS.txt', 'release-manifest.json')) {
    $stalePath = Join-Path $outputDir $stale
    if (Test-Path -LiteralPath $stalePath -PathType Leaf) { Remove-Item -LiteralPath $stalePath -Force }
}
$outputName = "RemoteDeck-$Version-win-x64-setup.exe"
$outputPath = Join-Path $outputDir $outputName
if (Test-Path -LiteralPath $outputPath) { Remove-Item -LiteralPath $outputPath -Force }

$defines = @(
    "/DAPP_VERSION=$Version",
    "/DAPP_BINARY=$(Convert-ToNsisPath $binary)",
    "/DAPP_ICON=$(Convert-ToNsisPath $icon)",
    "/DOUTFILE=$(Convert-ToNsisPath $outputPath)",
    "/DPRODUCT_NAME=RemoteDeck"
)
Write-Host "Compiling current-user NSIS installer with $makensis"
& $makensis @defines (Convert-ToNsisPath $nsi)
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE." }
if (-not (Test-Path -LiteralPath $outputPath -PathType Leaf)) {
    throw "makensis completed without producing $outputPath"
}

$installer = Get-Item -LiteralPath $outputPath
$hash = (Get-FileHash -LiteralPath $outputPath -Algorithm SHA256).Hash.ToLowerInvariant()
$signature = Get-AuthenticodeSignature -LiteralPath $outputPath
$manifest = [ordered]@{
    version = $Version
    platform = 'windows-x64'
    installer = $installer.Name
    sha256 = $hash
    bytes = $installer.Length
    authenticodeStatus = [string]$signature.Status
    unsigned = ($signature.Status -eq 'NotSigned')
    binary = 'RemoteDeck.exe'
    webAssets = 'embedded in RemoteDeck.exe'
    installerScope = 'currentUser'
    appData = '%APPDATA%\io.github.shilittle.remotedeck'
}
$manifestPath = Join-Path $outputDir 'release-manifest.json'
$checksumsPath = Join-Path $outputDir 'SHA256SUMS.txt'
$manifestJson = $manifest | ConvertTo-Json -Depth 5
Write-Utf8NoBom -Path $manifestPath -Content $manifestJson
Write-Utf8NoBom -Path $checksumsPath -Content "$hash  $($installer.Name)"
Write-Host "NSIS installer: $($installer.FullName)"
Write-Host "Installer bytes: $($installer.Length)"
Write-Host "Authenticode: $($signature.Status)"
